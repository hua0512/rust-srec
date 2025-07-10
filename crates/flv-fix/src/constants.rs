//! Constants for commonly used strings to avoid repeated allocations

// File extensions
pub const FLV_EXTENSION: &str = "flv";
pub const DEFAULT_FILENAME: &str = "file";

// Metadata property keys (used in AMF0 encoding)
pub const METADATA_FRAMERATE: &str = "framerate";
pub const METADATA_AUDIOSAMPLERATE: &str = "audiosamplerate";
pub const METADATA_DURATION: &str = "duration";
pub const METADATA_WIDTH: &str = "width";
pub const METADATA_HEIGHT: &str = "height";
pub const METADATA_VIDEOCODECID: &str = "videocodecid";
pub const METADATA_AUDIOCODECID: &str = "audiocodecid";
pub const METADATA_HAS_AUDIO: &str = "hasAudio";
pub const METADATA_HAS_VIDEO: &str = "hasVideo";
pub const METADATA_HAS_METADATA: &str = "hasMetadata";
pub const METADATA_HAS_KEYFRAMES: &str = "hasKeyframes";
pub const METADATA_CAN_SEEK_TO_END: &str = "canSeekToEnd";
pub const METADATA_DATASIZE: &str = "datasize";
pub const METADATA_FILESIZE: &str = "filesize";
pub const METADATA_AUDIOSIZE: &str = "audiosize";
pub const METADATA_AUDIODATARATE: &str = "audiodatarate";
pub const METADATA_AUDIOSAMPLESIZE: &str = "audiosamplesize";
pub const METADATA_STEREO: &str = "stereo";
pub const METADATA_VIDEOSIZE: &str = "videosize";
pub const METADATA_VIDEODATARATE: &str = "videodatarate";
pub const METADATA_LASTTIMESTAMP: &str = "lasttimestamp";
pub const METADATA_LASTKEYFRAMELOCATION: &str = "lastkeyframelocation";
pub const METADATA_LASTKEYFRAMETIMESTAMP: &str = "lastkeyframetimestamp";
pub const METADATA_METADATACREATOR: &str = "metadatacreator";
pub const METADATA_METADATADATE: &str = "metadatadate";
pub const METADATA_KEYFRAMES: &str = "keyframes";

// Keyframe property keys
pub const KEYFRAMES_TIMES: &str = "times";
pub const KEYFRAMES_FILEPOSITIONS: &str = "filepositions";

// Error messages
pub const ERROR_HEADER_ALREADY_ANALYZED: &str = "Header already analyzed";
pub const ERROR_HEADER_NOT_ANALYZED: &str = "Header not analyzed";
pub const ERROR_NO_ACTIVE_WRITER: &str = "Attempted write_tag with no active writer";

// AMF0 script data names
pub const AMF0_ON_METADATA: &str = "onMetaData";

// Default creator value
pub const DEFAULT_CREATOR: &str = "Srec";
