use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamFormat {
    Flv,
    Hls,
    Mp4,
    Wss,
}

impl StreamFormat {
    pub fn as_str(&self) -> &str {
        match self {
            StreamFormat::Flv => "flv",
            StreamFormat::Hls => "hls",
            StreamFormat::Mp4 => "mp4",
            StreamFormat::Wss => "wss",
        }
    }

    pub fn from_extension(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "flv" => StreamFormat::Flv,
            "m3u8" => StreamFormat::Hls,
            "mp4" => StreamFormat::Mp4,
            "wss" => StreamFormat::Wss,
            _ => StreamFormat::Flv,
        }
    }
}

impl Display for StreamFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for StreamFormat {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "flv" => Ok(StreamFormat::Flv),
            "hls" => Ok(StreamFormat::Hls),
            "mp4" => Ok(StreamFormat::Mp4),
            "wss" => Ok(StreamFormat::Wss),
            _ => Err(()),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaFormat {
    Flv,
    Ts,
    Mp4,
}

impl MediaFormat {
    pub fn as_str(&self) -> &str {
        match self {
            MediaFormat::Flv => "flv",
            MediaFormat::Ts => "ts",
            MediaFormat::Mp4 => "mp4",
        }
    }

    pub fn from_extension(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "flv" => MediaFormat::Flv,
            "ts" => MediaFormat::Ts,
            "fmp4" | "mp4" => MediaFormat::Mp4,
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
            "ts" => Ok(MediaFormat::Ts),
            "fmp4" | "mp4" => Ok(MediaFormat::Mp4),
            _ => Err(()),
        }
    }
}
