use amf0::{Amf0Value, Amf0WriteError};
use flv::{audio::SoundFormat, video::VideoCodecId};
use std::collections::HashMap;
use time::OffsetDateTime;

/// Represents the strongly-typed `onMetaData` object.
#[derive(Debug, Clone, Default)]
pub struct AmfScriptData {
    // Video Properties
    pub duration: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub framerate: Option<f64>,
    pub videocodecid: Option<VideoCodecId>,
    pub videodatarate: Option<f64>,

    // Audio Properties
    pub audiocodecid: Option<SoundFormat>,
    pub audiodatarate: Option<f64>,
    pub audiosamplerate: Option<f64>,
    pub audiosamplesize: Option<f64>,
    pub stereo: Option<bool>,

    // File Properties
    pub filesize: Option<u64>,
    pub datasize: Option<u64>,
    pub videosize: Option<u64>,
    pub audiosize: Option<u64>,
    pub lasttimestamp: Option<u32>,
    pub lastkeyframetimestamp: Option<u32>,
    pub lastkeyframelocation: Option<u64>,

    // Flags
    pub has_video: Option<bool>,
    pub has_audio: Option<bool>,
    pub has_metadata: Option<bool>,
    pub has_keyframes: Option<bool>,
    pub can_seek_to_end: Option<bool>,

    // Keyframes
    pub keyframes: Option<KeyframeData>,
    pub spacer_size: Option<usize>,

    // Metadata
    pub metadatacreator: Option<String>,
    pub metadatadate: Option<OffsetDateTime>,

    // Unknown or custom properties
    pub custom_properties: HashMap<String, Amf0Value<'static>>,
}

/// Represents the `keyframes` object within `onMetaData`.
#[derive(Debug, Clone)]
pub enum KeyframeData {
    /// For the `script_modifier` use case, with complete keyframe data.
    Final {
        times: Vec<f64>,
        filepositions: Vec<u64>,
    },
    /// For the `script_filler` use case, with placeholder arrays and a spacer.
    Placeholder { spacer_size: usize },
}

/// Extract f64 values from a StrictArray-like value.
fn extract_f64_array(value: &Amf0Value<'_>) -> Option<Vec<f64>> {
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(|v| v.as_number())
            .collect(),
    )
}

/// Extract u64 values (cast from f64) from a StrictArray-like value.
fn extract_u64_array(value: &Amf0Value<'_>) -> Option<Vec<u64>> {
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(|v| v.as_number().map(|n| n as u64))
            .collect(),
    )
}

impl AmfScriptData {
    pub fn from_amf_object_ref(
        obj: &[(impl AsRef<str>, Amf0Value<'_>)],
    ) -> Result<Self, Amf0WriteError> {
        let mut data = AmfScriptData::default();
        let mut custom_properties = HashMap::new();

        for (key, value) in obj {
            match key.as_ref() {
                "duration" => data.duration = value.as_number(),
                "width" => data.width = value.as_number(),
                "height" => data.height = value.as_number(),
                "framerate" => data.framerate = value.as_number(),
                "videocodecid" => {
                    data.videocodecid = value
                        .as_number()
                        .and_then(|v| VideoCodecId::try_from(v as u8).ok())
                }
                "videodatarate" => data.videodatarate = value.as_number(),
                "audiocodecid" => {
                    data.audiocodecid = value
                        .as_number()
                        .and_then(|v| SoundFormat::try_from(v as u8).ok())
                }
                "audiodatarate" => data.audiodatarate = value.as_number(),
                "audiosamplerate" => data.audiosamplerate = value.as_number(),
                "audiosamplesize" => data.audiosamplesize = value.as_number(),
                "stereo" => data.stereo = value.as_bool(),
                "filesize" => data.filesize = value.as_number().map(|v| v as u64),
                "datasize" => data.datasize = value.as_number().map(|v| v as u64),
                "videosize" => data.videosize = value.as_number().map(|v| v as u64),
                "audiosize" => data.audiosize = value.as_number().map(|v| v as u64),
                "lasttimestamp" => data.lasttimestamp = value.as_number().map(|v| v as u32),
                "lastkeyframetimestamp" => {
                    data.lastkeyframetimestamp = value.as_number().map(|v| v as u32)
                }
                "lastkeyframelocation" => {
                    data.lastkeyframelocation = value.as_number().map(|v| v as u64)
                }
                "hasVideo" => data.has_video = value.as_bool(),
                "hasAudio" => data.has_audio = value.as_bool(),
                "hasMetadata" => data.has_metadata = value.as_bool(),
                "hasKeyframes" => data.has_keyframes = value.as_bool(),
                "canSeekToEnd" => data.can_seek_to_end = value.as_bool(),
                "creationdate" => {
                    data.metadatadate = value.as_str().and_then(|s| {
                        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                            .ok()
                    })
                }
                "metadatacreator" => data.metadatacreator = value.as_str().map(|s| s.to_string()),
                "keyframes" => {
                    if let Some(props) = value.as_object_properties() {
                        let mut times = None;
                        let mut filepositions = None;
                        let mut spacer_size = None;

                        for (k, v) in props {
                            match k.as_ref() {
                                "times" if times.is_none() => times = extract_f64_array(v),
                                "filepositions" if filepositions.is_none() => {
                                    filepositions = extract_u64_array(v)
                                }
                                "spacer" if spacer_size.is_none() => {
                                    spacer_size = v.as_array().map(|a| a.len())
                                }
                                _ => {}
                            }
                        }

                        data.spacer_size = spacer_size;

                        if let (Some(times), Some(filepositions)) = (times, filepositions) {
                            data.keyframes = Some(KeyframeData::Final {
                                times,
                                filepositions,
                            });
                        }
                    }
                }
                _ => {
                    custom_properties.insert(key.as_ref().to_string(), value.into_owned());
                }
            }
        }

        data.custom_properties = custom_properties;
        Ok(data)
    }
}
