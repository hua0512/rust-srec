use amf0::{Amf0Value, Amf0WriteError};
use flv::{
    audio::{AudioCodec, AudioFourCC},
    video::{VideoCodec, VideoFourCC},
};
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
    pub videocodecid: Option<VideoCodec>,
    pub videodatarate: Option<f64>,

    // Audio Properties
    pub audiocodecid: Option<AudioCodec>,
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

fn parse_video_codec(value: &Amf0Value<'_>) -> Option<VideoCodec> {
    if let Some(value) = value.as_number()
        && value.is_finite()
        && value.fract() == 0.0
        && (0.0..=u32::MAX as f64).contains(&value)
    {
        return VideoCodec::try_from(value as u32).ok();
    }

    // Some origins emit a FourCC string despite Enhanced RTMP requiring a number.
    let bytes = <[u8; 4]>::try_from(value.as_str()?.as_bytes()).ok()?;
    VideoFourCC::try_from(bytes).ok().map(VideoCodec::Enhanced)
}

fn parse_audio_codec(value: &Amf0Value<'_>) -> Option<AudioCodec> {
    if let Some(value) = value.as_number()
        && value.is_finite()
        && value.fract() == 0.0
        && (0.0..=u32::MAX as f64).contains(&value)
    {
        return AudioCodec::try_from(value as u32).ok();
    }

    // Some origins emit a FourCC string despite Enhanced RTMP requiring a number.
    let bytes = <[u8; 4]>::try_from(value.as_str()?.as_bytes()).ok()?;
    AudioFourCC::from_u32(u32::from_be_bytes(bytes))
        .ok()
        .map(AudioCodec::Enhanced)
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
                "videocodecid" => data.videocodecid = parse_video_codec(value),
                "videodatarate" => data.videodatarate = value.as_number(),
                "audiocodecid" => data.audiocodecid = parse_audio_codec(value),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nonstandard_string_video_fourcc() {
        assert_eq!(
            parse_video_codec(&Amf0Value::String("av01".into())),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01))
        );
    }

    #[test]
    fn rejects_invalid_numeric_video_codecs_without_lossy_casts() {
        for value in [-1.0, 7.5, u32::MAX as f64 + 1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(parse_video_codec(&Amf0Value::Number(value)), None);
        }
    }

    #[test]
    fn parses_legacy_and_fourcc_audio_codecs() {
        use flv::audio::SoundFormat;

        assert_eq!(
            parse_audio_codec(&Amf0Value::Number(10.0)),
            Some(AudioCodec::Legacy(SoundFormat::Aac))
        );
        assert_eq!(
            parse_audio_codec(&Amf0Value::Number(AudioFourCC::Opus.as_u32() as f64)),
            Some(AudioCodec::Enhanced(AudioFourCC::Opus))
        );
        assert_eq!(
            parse_audio_codec(&Amf0Value::String("Opus".into())),
            Some(AudioCodec::Enhanced(AudioFourCC::Opus))
        );
    }

    #[test]
    fn rejects_invalid_numeric_audio_codecs_without_lossy_casts() {
        for value in [-1.0, 10.5, u32::MAX as f64 + 1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(parse_audio_codec(&Amf0Value::Number(value)), None);
        }
    }
}
