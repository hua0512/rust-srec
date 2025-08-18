use crate::amf::model::{AmfScriptData, KeyframeData};
use crate::analyzer::FlvStats;
use amf0::{Amf0Encoder, Amf0Marker, Amf0Value, Amf0WriteError, write_amf_property_key};
use byteorder::{BigEndian, WriteBytesExt};
use flv::{audio::SoundFormat, video::VideoCodecId};
use std::borrow::Cow;
use std::io::Write;
use tracing::debug;

const NATURAL_METADATA_KEY_ORDER: &[&str] = &[
    "duration",
    "width",
    "height",
    "videodatarate",
    "framerate",
    "videocodecid",
    "audiodatarate",
    "audiosamplerate",
    "audiosamplesize",
    "stereo",
    "audiocodecid",
    "filesize",
    "datasize",
    "videosize",
    "audiosize",
    "lasttimestamp",
    "lastkeyframetimestamp",
    "lastkeyframelocation",
    "hasVideo",
    "hasAudio",
    "hasMetadata",
    "hasKeyframes",
    "canSeekToEnd",
    "metadatacreator",
    "metadatadate",
    "keyframes",
];

/// A fluent builder for creating `onMetaData` script data.
#[derive(Debug, Default)]
pub struct OnMetaDataBuilder {
    data: AmfScriptData,
}

impl OnMetaDataBuilder {
    /// Creates a new builder.
    pub fn new() -> Self {
        Self {
            data: AmfScriptData::default(),
        }
    }

    /// Creates a new builder from an existing `AmfScriptData` object,
    /// preserving any custom properties.
    pub fn from_script_data(data: AmfScriptData) -> Self {
        Self { data }
    }

    /// Populates the builder with data from an `FlvStats` object.
    pub fn with_stats(mut self, stats: &FlvStats) -> Self {
        self.data.duration = Some(stats.duration as f64);
        if let Some(res) = &stats.resolution {
            self.data.width = Some(res.width as f64);
            self.data.height = Some(res.height as f64);
        }
        self.data.framerate = Some(stats.video_frame_rate as f64);
        self.data.videocodecid = stats.video_codec;
        self.data.audiocodecid = stats.audio_codec;
        self.data.filesize = Some(stats.file_size);
        self.data.videosize = Some(stats.video_data_size);
        self.data.audiosize = Some(stats.audio_data_size);
        self.data.lasttimestamp = Some(stats.last_timestamp);
        self.data.lastkeyframetimestamp = Some(stats.last_keyframe_timestamp);
        self.data.lastkeyframelocation = Some(stats.last_keyframe_position);
        self.data.has_video = Some(stats.has_video);
        self.data.has_audio = Some(stats.has_audio);
        self.data.has_metadata = Some(true);
        self.data.has_keyframes = Some(!stats.keyframes.is_empty());
        self.data.can_seek_to_end = Some(stats.last_keyframe_timestamp == stats.last_timestamp);
        self
    }

    /// Sets the duration in seconds.
    pub fn with_duration(mut self, duration: f64) -> Self {
        self.data.duration = Some(duration);
        self
    }

    /// Sets the video width.
    pub fn with_width(mut self, width: f64) -> Self {
        self.data.width = Some(width);
        self
    }

    /// Sets the video height.
    pub fn with_height(mut self, height: f64) -> Self {
        self.data.height = Some(height);
        self
    }

    /// Sets the video framerate.
    pub fn with_framerate(mut self, framerate: f64) -> Self {
        self.data.framerate = Some(framerate);
        self
    }

    /// Sets the video codec ID.
    pub fn with_video_codec(mut self, codec: VideoCodecId) -> Self {
        self.data.videocodecid = Some(codec);
        self
    }

    /// Sets the audio codec ID.
    pub fn with_audio_codec(mut self, codec: SoundFormat) -> Self {
        self.data.audiocodecid = Some(codec);
        self
    }

    /// Sets a custom property.
    pub fn with_custom_property(
        mut self,
        key: impl Into<String>,
        value: Amf0Value<'static>,
    ) -> Self {
        self.data.custom_properties.insert(key.into(), value);
        self
    }

    /// Configures the builder to generate a complete `keyframes` object.
    pub fn with_final_keyframes(mut self, times: Vec<f64>, filepositions: Vec<u64>) -> Self {
        self.data.keyframes = Some(KeyframeData::Final {
            times,
            filepositions,
        });
        self
    }

    /// Configures the builder to generate a `keyframes` object with placeholders.
    pub fn with_placeholder_keyframes(mut self, spacer_size: usize) -> Self {
        self.data.keyframes = Some(KeyframeData::Placeholder { spacer_size });
        self
    }

    /// Builds the final `AmfScriptData` model.
    pub fn build_model(self) -> AmfScriptData {
        self.data
    }

    /// Builds the AMF0 byte payload, ready for writing to a file.
    ///
    /// # Arguments
    ///
    /// * `original_payload_size` - The size of the original script data payload.
    ///   This is used to calculate the size difference and adjust keyframe file positions.
    /// * `low_latency_mode` - Whether to use low-latency mode for metadata modification.
    ///   This will reduce the latency of script data modification, but it will also increase the size of the output file.
    ///
    /// # Returns
    ///
    /// A tuple containing the new byte payload and the size difference.
    pub fn build_bytes(
        mut self,
        original_payload_size: u32,
        low_latency_mode: bool,
    ) -> Result<(Vec<u8>, i64), Amf0WriteError> {
        // arbitrary buffer size
        let estimated_size = original_payload_size as usize + 128;
        let mut buf = Vec::with_capacity(estimated_size);

        // start the main object
        Amf0Encoder::encode_string(&mut buf, "onMetaData")?;
        buf.write_u8(Amf0Marker::Object as u8)?;

        // encode all properties *except* keyframes
        for &key in NATURAL_METADATA_KEY_ORDER {
            if key == "keyframes" {
                continue;
            }
            if let Some(value) = self.get_amf_value_for_key(key) {
                write_amf_property_key!(&mut buf, key);
                Amf0Encoder::encode(&mut buf, &value)?;
            }
        }

        for (key, value) in &self.data.custom_properties {
            write_amf_property_key!(&mut buf, key);
            Amf0Encoder::encode(&mut buf, value)?;
        }

        // calculate the size difference
        let metadata_size_without_keyframes = buf.len() + 3; // +3 for object_eof

        let file_positions_size = self
            .data
            .keyframes
            .as_ref()
            .map(|k| match k {
                KeyframeData::Final { filepositions, .. } => filepositions.len(),
                _ => 0,
            })
            .unwrap_or(0);

        let times_size = self
            .data
            .keyframes
            .as_ref()
            .map(|k| match k {
                KeyframeData::Final { times, .. } => times.len(),
                _ => 0,
            })
            .unwrap_or(0);

        let spacer_size = self.data.spacer_size.unwrap_or(0);

        debug!(
            "file_positions_size={}, times_size={}, spacer_size={}",
            file_positions_size, times_size, spacer_size
        );

        // if we are on low-latency mode and the spacer size is greater than the file positions and times size,
        // we can use the original payload size, and pad the remaining space with 0s
        // requirement: ScriptFiller operator must be used in the pipeline
        let keyframes_size = if low_latency_mode
            && spacer_size > 0
            && file_positions_size + times_size <= spacer_size
        {
            original_payload_size as usize - metadata_size_without_keyframes
        } else {
            self.data
                .keyframes
                .as_ref()
                .map(Self::compute_keyframes_section_size)
                .unwrap_or(0)
        };

        debug!("keyframes_size={}", keyframes_size);

        let final_size = metadata_size_without_keyframes + keyframes_size;
        let size_diff = final_size as i64 - original_payload_size as i64;

        debug!(
            "Size calculation: metadata_without_keyframes={}, keyframes_size={}, final_size={}, original_payload_size={}, size_diff={}",
            metadata_size_without_keyframes,
            keyframes_size,
            final_size,
            original_payload_size,
            size_diff
        );

        // encode the keyframes object with the correct offsets
        if let Some(keyframes) = self.data.keyframes.take() {
            self.write_keyframes_section(&mut buf, keyframes, size_diff, low_latency_mode)?;
        }

        // finalize the main object
        Amf0Encoder::object_eof(&mut buf)?;

        Ok((buf, size_diff))
    }

    /// Returns the AMF0 value for a given key.
    /// If the key is not found, a default value is returned.
    /// Returns `None` if the key is not known.
    fn get_amf_value_for_key(&mut self, key: &str) -> Option<Amf0Value<'static>> {
        match key {
            "duration" => self
                .data
                .duration
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "width" => self
                .data
                .width
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "height" => self
                .data
                .height
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "framerate" => self
                .data
                .framerate
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "videocodecid" => self
                .data
                .videocodecid
                .map(|v| Amf0Value::Number(v as u8 as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "videodatarate" => self
                .data
                .videodatarate
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "audiocodecid" => self
                .data
                .audiocodecid
                .map(|v| Amf0Value::Number(v as u8 as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "audiodatarate" => self
                .data
                .audiodatarate
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "audiosamplerate" => self
                .data
                .audiosamplerate
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "audiosamplesize" => self
                .data
                .audiosamplesize
                .map(Amf0Value::Number)
                .or(Some(Amf0Value::Number(0.0))),
            "stereo" => self
                .data
                .stereo
                .map(Amf0Value::Boolean)
                .or(Some(Amf0Value::Boolean(false))),
            "filesize" => self
                .data
                .filesize
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "datasize" => self
                .data
                .datasize
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "videosize" => self
                .data
                .videosize
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "audiosize" => self
                .data
                .audiosize
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "lasttimestamp" => self
                .data
                .lasttimestamp
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "lastkeyframetimestamp" => self
                .data
                .lastkeyframetimestamp
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "lastkeyframelocation" => self
                .data
                .lastkeyframelocation
                .map(|v| Amf0Value::Number(v as f64))
                .or(Some(Amf0Value::Number(0.0))),
            "hasVideo" => self
                .data
                .has_video
                .map(Amf0Value::Boolean)
                .or(Some(Amf0Value::Boolean(false))),
            "hasAudio" => self
                .data
                .has_audio
                .map(Amf0Value::Boolean)
                .or(Some(Amf0Value::Boolean(false))),
            "hasMetadata" => self
                .data
                .has_metadata
                .map(Amf0Value::Boolean)
                .or(Some(Amf0Value::Boolean(false))),
            "hasKeyframes" => self
                .data
                .has_keyframes
                .map(Amf0Value::Boolean)
                .or(Some(Amf0Value::Boolean(false))),
            "canSeekToEnd" => self
                .data
                .can_seek_to_end
                .map(Amf0Value::Boolean)
                .or(Some(Amf0Value::Boolean(false))),
            "metadatacreator" => self
                .data
                .metadatacreator
                .take()
                .map(|v| Amf0Value::String(Cow::Owned(v)))
                .or(Some(Amf0Value::String(Cow::Borrowed(concat!(
                    "Mesio v",
                    env!("CARGO_PKG_VERSION")
                ))))),
            _ => None,
        }
    }

    /// Computes the size of the keyframes section.
    fn compute_keyframes_section_size(keyframes: &KeyframeData) -> usize {
        let mut size = 0;
        // "keyframes" property key + object marker
        size += "keyframes".len() + 2 + 1;

        match keyframes {
            KeyframeData::Final {
                times,
                filepositions: _,
            } => {
                let keyframes_length = times.len();
                // "times" property key + strict array marker + array length + (marker + f64) * count
                size += "times".len() + 2 + 1 + 4 + (keyframes_length * 9);
                // "filepositions" property key + strict array marker + array length + (marker + f64) * count
                size += "filepositions".len() + 2 + 1 + 4 + (keyframes_length * 9);
            }
            KeyframeData::Placeholder { spacer_size } => {
                // "times" property key + strict array marker + array length (0)
                size += "times".len() + 2 + 1 + 4;
                // "filepositions" property key + strict array marker + array length (0)
                size += "filepositions".len() + 2 + 1 + 4;
                if *spacer_size > 0 {
                    // "spacer" property key + strict array marker + array length + (marker + f64) * count
                    size += "spacer".len() + 2 + 1 + 4 + (*spacer_size * 9);
                }
            }
        }

        // Object EOF marker
        size += 3;
        size
    }

    /// Writes the keyframes section to the buffer.
    /// * `buf` - The buffer to write the keyframes section to.
    /// * `keyframes` - The keyframes data.
    /// * `size_diff` - The size difference between the original and modified keyframes section.
    /// * `low_latency_mode` - Whether to use low-latency mode for metadata modification.
    ///   This will reduce the latency of script data modification, but it will also increase the size of the output file.
    fn write_keyframes_section(
        self,
        buf: &mut Vec<u8>,
        keyframes: KeyframeData,
        size_diff: i64,
        low_latency_mode: bool,
    ) -> Result<(), Amf0WriteError> {
        let start_position = buf.len();
        write_amf_property_key!(buf, "keyframes");
        buf.write_u8(Amf0Marker::Object as u8)?;

        match keyframes {
            KeyframeData::Final {
                times,
                filepositions,
            } => {
                // Write times array
                write_amf_property_key!(buf, "times");
                buf.write_u8(Amf0Marker::StrictArray as u8)?;
                buf.write_u32::<BigEndian>(times.len() as u32)?;
                for time in &times {
                    Amf0Encoder::encode_number(buf, *time)?;
                }

                // Write filepositions array
                write_amf_property_key!(buf, "filepositions");
                buf.write_u8(Amf0Marker::StrictArray as u8)?;
                buf.write_u32::<BigEndian>(filepositions.len() as u32)?;
                for pos in &filepositions {
                    let adjusted_pos = (*pos as i64 + size_diff) as f64;
                    Amf0Encoder::encode_number(buf, adjusted_pos)?;
                }

                let current_filepositions_size = filepositions.len();
                let current_times_size = times.len();

                let spacer_size = self.data.spacer_size.unwrap_or(0);
                debug!(
                    "current_filepositions_size={current_filepositions_size}, current_times_size={current_times_size}, spacer_size={spacer_size}"
                );

                if low_latency_mode
                    && spacer_size > 0
                    && current_filepositions_size + current_times_size <= spacer_size
                {
                    let remaining_spacer_size =
                        spacer_size - current_filepositions_size - current_times_size;
                    debug!("Remaining spacer size: {remaining_spacer_size}");
                    if remaining_spacer_size > 0 {
                        // Write spacer array
                        write_amf_property_key!(buf, "spacer");
                        buf.write_u8(Amf0Marker::StrictArray as u8)?;
                        buf.write_u32::<BigEndian>(remaining_spacer_size as u32)?;
                        for _ in 0..remaining_spacer_size {
                            Amf0Encoder::encode_number(buf, 0.0)?;
                        }
                    }
                }
            }
            KeyframeData::Placeholder { spacer_size } => {
                // Write empty times array
                write_amf_property_key!(buf, "times");
                buf.write_u8(Amf0Marker::StrictArray as u8)?;
                buf.write_u32::<BigEndian>(0)?;

                // Write empty filepositions array
                write_amf_property_key!(buf, "filepositions");
                buf.write_u8(Amf0Marker::StrictArray as u8)?;
                buf.write_u32::<BigEndian>(0)?;

                if spacer_size > 0 {
                    write_amf_property_key!(buf, "spacer");
                    buf.write_u8(Amf0Marker::StrictArray as u8)?;
                    buf.write_u32::<BigEndian>(spacer_size as u32)?;
                    for _ in 0..spacer_size {
                        Amf0Encoder::encode_number(buf, 0.0)?;
                    }
                }
            }
        }

        Amf0Encoder::object_eof(buf)?;
        let end_position = buf.len();
        debug!("keyframes section size: {}", end_position - start_position);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amf0::Amf0Decoder;

    #[test]
    fn test_on_meta_data_builder_final_keyframes() {
        let builder = OnMetaDataBuilder::new()
            .with_duration(10.0)
            .with_width(1920.0)
            .with_height(1080.0)
            .with_final_keyframes(vec![0.0, 1.0, 2.0], vec![100, 200, 300]);

        let (bytes, size_diff) = builder.build_bytes(0, false).unwrap();

        assert_eq!(size_diff, bytes.len() as i64);

        let mut decoder = Amf0Decoder::new(&bytes);
        let name = decoder.decode().unwrap();
        let data = decoder.decode().unwrap();

        assert_eq!(name, Amf0Value::String(Cow::Borrowed("onMetaData")));
        if let Amf0Value::Object(props) = data {
            // The number of properties should be the number of keys in NATURAL_METADATA_KEY_ORDER
            // minus "metadatadate" (which is not implemented).
            let expected_len = NATURAL_METADATA_KEY_ORDER
                .iter()
                .filter(|&&key| key != "metadatadate")
                .count();
            assert_eq!(props.len(), expected_len);

            // Check that the first 3 properties are in order
            assert_eq!(props[0].0, "duration");
            assert_eq!(props[0].1, Amf0Value::Number(10.0));
            assert_eq!(props[1].0, "width");
            assert_eq!(props[1].1, Amf0Value::Number(1920.0));
            assert_eq!(props[2].0, "height");
            assert_eq!(props[2].1, Amf0Value::Number(1080.0));

            // Find the keyframes property
            let keyframes_prop = props.iter().find(|(k, _)| k == "keyframes").unwrap();
            assert_eq!(keyframes_prop.0, "keyframes");
            if let Amf0Value::Object(keyframes) = &keyframes_prop.1 {
                assert_eq!(keyframes.len(), 2);
                assert_eq!(keyframes[0].0, "times");
                if let Amf0Value::StrictArray(times) = &keyframes[0].1 {
                    assert_eq!(times.len(), 3);
                    assert_eq!(times[0], Amf0Value::Number(0.0));
                    assert_eq!(times[1], Amf0Value::Number(1.0));
                    assert_eq!(times[2], Amf0Value::Number(2.0));
                } else {
                    panic!("Expected strict array for times");
                }

                assert_eq!(keyframes[1].0, "filepositions");
                if let Amf0Value::StrictArray(filepositions) = &keyframes[1].1 {
                    assert_eq!(filepositions.len(), 3);
                    assert_eq!(
                        filepositions[0],
                        Amf0Value::Number(100.0 + size_diff as f64)
                    );
                    assert_eq!(
                        filepositions[1],
                        Amf0Value::Number(200.0 + size_diff as f64)
                    );
                    assert_eq!(
                        filepositions[2],
                        Amf0Value::Number(300.0 + size_diff as f64)
                    );
                } else {
                    panic!("Expected strict array for filepositions");
                }
            } else {
                panic!("Expected object for keyframes");
            }
        } else {
            panic!("Expected object for metadata");
        }
    }

    #[test]
    fn test_on_meta_data_builder_placeholder_keyframes() {
        let builder = OnMetaDataBuilder::new()
            .with_duration(10.0)
            .with_width(1920.0)
            .with_height(1080.0)
            .with_placeholder_keyframes(1024);

        let (bytes, size_diff) = builder.build_bytes(0, false).unwrap();

        assert_eq!(size_diff, bytes.len() as i64);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_on_meta_data_builder_filepositions_offset() {
        let original_payload_size = 100;
        let keyframes_pos = vec![1000, 2000, 3000];

        let builder = OnMetaDataBuilder::new()
            .with_duration(10.0)
            .with_final_keyframes(vec![0.0, 1.0, 2.0], keyframes_pos.clone());

        let (bytes, size_diff) = builder.build_bytes(original_payload_size, false).unwrap();

        assert_eq!(size_diff, bytes.len() as i64 - original_payload_size as i64);

        let mut decoder = Amf0Decoder::new(&bytes);
        let _name = decoder.decode().unwrap();
        let data = decoder.decode().unwrap();

        if let Amf0Value::Object(props) = data {
            let keyframes_prop = props.iter().find(|(k, _)| k == "keyframes").unwrap();
            if let Amf0Value::Object(keyframes) = &keyframes_prop.1 {
                let filepositions_prop = keyframes
                    .iter()
                    .find(|(k, _)| k == "filepositions")
                    .unwrap();
                if let Amf0Value::StrictArray(filepositions) = &filepositions_prop.1 {
                    assert_eq!(filepositions.len(), 3);
                    assert_eq!(
                        filepositions[0],
                        Amf0Value::Number((keyframes_pos[0] as i64 + size_diff) as f64)
                    );
                    assert_eq!(
                        filepositions[1],
                        Amf0Value::Number((keyframes_pos[1] as i64 + size_diff) as f64)
                    );
                    assert_eq!(
                        filepositions[2],
                        Amf0Value::Number((keyframes_pos[2] as i64 + size_diff) as f64)
                    );
                } else {
                    panic!("Expected strict array for filepositions");
                }
            } else {
                panic!("Expected object for keyframes");
            }
        } else {
            panic!("Expected object for metadata");
        }
    }
}
