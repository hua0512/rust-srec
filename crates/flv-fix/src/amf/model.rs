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

impl AmfScriptData {
    pub fn from_amf_object(obj: &mut Vec<(String, Amf0Value<'_>)>) -> Result<Self, Amf0WriteError> {
        let mut data = AmfScriptData::default();
        let mut custom_properties = HashMap::new();

        for (key, value) in obj.drain(..) {
            match key.as_str() {
                "duration" => {
                    if let Amf0Value::Number(v) = value {
                        data.duration = Some(v)
                    }
                }
                "width" => {
                    if let Amf0Value::Number(v) = value {
                        data.width = Some(v)
                    }
                }
                "height" => {
                    if let Amf0Value::Number(v) = value {
                        data.height = Some(v)
                    }
                }
                "framerate" => {
                    if let Amf0Value::Number(v) = value {
                        data.framerate = Some(v)
                    }
                }
                "videocodecid" => {
                    if let Amf0Value::Number(v) = value {
                        data.videocodecid = VideoCodecId::try_from(v as u8).ok()
                    }
                }
                "videodatarate" => {
                    if let Amf0Value::Number(v) = value {
                        data.videodatarate = Some(v)
                    }
                }
                "audiocodecid" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiocodecid = SoundFormat::try_from(v as u8).ok()
                    }
                }
                "audiodatarate" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiodatarate = Some(v)
                    }
                }
                "audiosamplerate" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiosamplerate = Some(v)
                    }
                }
                "audiosamplesize" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiosamplesize = Some(v)
                    }
                }
                "stereo" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.stereo = Some(v)
                    }
                }
                "filesize" => {
                    if let Amf0Value::Number(v) = value {
                        data.filesize = Some(v as u64)
                    }
                }
                "datasize" => {
                    if let Amf0Value::Number(v) = value {
                        data.datasize = Some(v as u64)
                    }
                }
                "videosize" => {
                    if let Amf0Value::Number(v) = value {
                        data.videosize = Some(v as u64)
                    }
                }
                "audiosize" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiosize = Some(v as u64)
                    }
                }
                "lasttimestamp" => {
                    if let Amf0Value::Number(v) = value {
                        data.lasttimestamp = Some(v as u32)
                    }
                }
                "lastkeyframetimestamp" => {
                    if let Amf0Value::Number(v) = value {
                        data.lastkeyframetimestamp = Some(v as u32)
                    }
                }
                "lastkeyframelocation" => {
                    if let Amf0Value::Number(v) = value {
                        data.lastkeyframelocation = Some(v as u64)
                    }
                }
                "hasVideo" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_video = Some(v)
                    }
                }
                "hasAudio" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_audio = Some(v)
                    }
                }
                "hasMetadata" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_metadata = Some(v)
                    }
                }
                "hasKeyframes" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_keyframes = Some(v)
                    }
                }
                "canSeekToEnd" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.can_seek_to_end = Some(v)
                    }
                }
                "metadatacreator" => {
                    if let Amf0Value::String(v) = value {
                        data.metadatacreator = Some(v.to_string())
                    }
                }
                "keyframes" => {
                    if let Amf0Value::Object(props) = value {
                        let times = props.iter().find(|(k, _)| k == "times").and_then(|(_, v)| {
                            if let Amf0Value::StrictArray(arr) = v {
                                Some(
                                    arr.iter()
                                        .filter_map(|v| {
                                            if let Amf0Value::Number(n) = v {
                                                Some(*n)
                                            } else {
                                                None
                                            }
                                        })
                                        .collect(),
                                )
                            } else {
                                None
                            }
                        });

                        let filepositions = props
                            .iter()
                            .find(|(k, _)| k == "filepositions")
                            .and_then(|(_, v)| {
                                if let Amf0Value::StrictArray(arr) = v {
                                    Some(
                                        arr.iter()
                                            .filter_map(|v| {
                                                if let Amf0Value::Number(n) = v {
                                                    Some(*n as u64)
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect(),
                                    )
                                } else {
                                    None
                                }
                            });

                        if let (Some(times), Some(filepositions)) = (times, filepositions) {
                            data.keyframes = Some(KeyframeData::Final {
                                times,
                                filepositions,
                            });
                        }
                    }
                }
                _ => {
                    custom_properties.insert(key, value.to_owned());
                }
            }
        }

        data.custom_properties = custom_properties;
        Ok(data)
    }

    pub fn from_amf_object_ref(
        obj: &[(impl AsRef<str>, Amf0Value<'_>)],
    ) -> Result<Self, Amf0WriteError> {
        let mut data = AmfScriptData::default();
        let mut custom_properties = HashMap::new();

        for (key, value) in obj {
            match key.as_ref() {
                "duration" => {
                    if let Amf0Value::Number(v) = value {
                        data.duration = Some(*v)
                    }
                }
                "width" => {
                    if let Amf0Value::Number(v) = value {
                        data.width = Some(*v)
                    }
                }
                "height" => {
                    if let Amf0Value::Number(v) = value {
                        data.height = Some(*v)
                    }
                }
                "framerate" => {
                    if let Amf0Value::Number(v) = value {
                        data.framerate = Some(*v)
                    }
                }
                "videocodecid" => {
                    if let Amf0Value::Number(v) = value {
                        data.videocodecid = VideoCodecId::try_from(*v as u8).ok()
                    }
                }
                "videodatarate" => {
                    if let Amf0Value::Number(v) = value {
                        data.videodatarate = Some(*v)
                    }
                }
                "audiocodecid" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiocodecid = SoundFormat::try_from(*v as u8).ok()
                    }
                }
                "audiodatarate" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiodatarate = Some(*v)
                    }
                }
                "audiosamplerate" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiosamplerate = Some(*v)
                    }
                }
                "audiosamplesize" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiosamplesize = Some(*v)
                    }
                }
                "stereo" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.stereo = Some(*v)
                    }
                }
                "filesize" => {
                    if let Amf0Value::Number(v) = value {
                        data.filesize = Some(*v as u64)
                    }
                }
                "datasize" => {
                    if let Amf0Value::Number(v) = value {
                        data.datasize = Some(*v as u64)
                    }
                }
                "videosize" => {
                    if let Amf0Value::Number(v) = value {
                        data.videosize = Some(*v as u64)
                    }
                }
                "audiosize" => {
                    if let Amf0Value::Number(v) = value {
                        data.audiosize = Some(*v as u64)
                    }
                }
                "lasttimestamp" => {
                    if let Amf0Value::Number(v) = value {
                        data.lasttimestamp = Some(*v as u32)
                    }
                }
                "lastkeyframetimestamp" => {
                    if let Amf0Value::Number(v) = value {
                        data.lastkeyframetimestamp = Some(*v as u32)
                    }
                }
                "lastkeyframelocation" => {
                    if let Amf0Value::Number(v) = value {
                        data.lastkeyframelocation = Some(*v as u64)
                    }
                }
                "hasVideo" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_video = Some(*v)
                    }
                }
                "hasAudio" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_audio = Some(*v)
                    }
                }
                "hasMetadata" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_metadata = Some(*v)
                    }
                }
                "hasKeyframes" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.has_keyframes = Some(*v)
                    }
                }
                "canSeekToEnd" => {
                    if let Amf0Value::Boolean(v) = value {
                        data.can_seek_to_end = Some(*v)
                    }
                }
                "metadatacreator" => {
                    if let Amf0Value::String(v) = value {
                        data.metadatacreator = Some(v.to_string())
                    }
                }
                "keyframes" => {
                    if let Amf0Value::Object(props) = value {
                        let times =
                            props
                                .iter()
                                .find(|(k, _)| k.as_ref() == "times")
                                .and_then(|(_, v)| {
                                    if let Amf0Value::StrictArray(arr) = v {
                                        Some(
                                            arr.iter()
                                                .filter_map(|v| {
                                                    if let Amf0Value::Number(n) = v {
                                                        Some(*n)
                                                    } else {
                                                        None
                                                    }
                                                })
                                                .collect(),
                                        )
                                    } else {
                                        None
                                    }
                                });

                        let filepositions = props
                            .iter()
                            .find(|(k, _)| k.as_ref() == "filepositions")
                            .and_then(|(_, v)| {
                                if let Amf0Value::StrictArray(arr) = v {
                                    Some(
                                        arr.iter()
                                            .filter_map(|v| {
                                                if let Amf0Value::Number(n) = v {
                                                    Some(*n as u64)
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect(),
                                    )
                                } else {
                                    None
                                }
                            });

                        if let (Some(times), Some(filepositions)) = (times, filepositions) {
                            data.keyframes = Some(KeyframeData::Final {
                                times,
                                filepositions,
                            });
                        }
                    }
                }
                _ => {
                    custom_properties.insert(key.as_ref().to_string(), value.to_owned());
                }
            }
        }

        data.custom_properties = custom_properties;
        Ok(data)
    }
}
