use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaFormat {
    Flv,
    Hls,
    Mp4,
}

impl MediaFormat {
    pub fn as_str(&self) -> &str {
        match self {
            MediaFormat::Flv => "flv",
            MediaFormat::Hls => "hls",
            MediaFormat::Mp4 => "mp4",
        }
    }

    pub fn from_extension(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "flv" => MediaFormat::Flv,
            "ts" | "m3u8" | "fmp4" => MediaFormat::Hls,
            "mp4" => MediaFormat::Mp4,
            _ => MediaFormat::Flv,
        }
    }
}

impl Display for MediaFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for MediaFormat {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "flv" => Ok(MediaFormat::Flv),
            "hls" => Ok(MediaFormat::Hls),
            "mp4" => Ok(MediaFormat::Mp4),
            _ => Err(()),
        }
    }
}
