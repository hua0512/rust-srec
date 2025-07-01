use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaFormat {
    Flv,
    Hls,
}

impl MediaFormat {
    pub fn as_str(&self) -> &str {
        match self {
            MediaFormat::Flv => "flv",
            MediaFormat::Hls => "hls",
        }
    }

    pub fn from_str(format: &str) -> Option<Self> {
        match format.to_lowercase().as_str() {
            "flv" => Some(MediaFormat::Flv),
            "hls" => Some(MediaFormat::Hls),
            _ => None,
        }
    }
}
