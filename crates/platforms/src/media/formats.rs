use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum StreamFormat {
    Flv,
    Hls,
    Mp4,
    Wss,
    /// MPEG-DASH: the URL is a Media Presentation Description (MPD).
    Dash,
}

impl StreamFormat {
    pub fn as_str(&self) -> &str {
        match self {
            StreamFormat::Flv => "flv",
            StreamFormat::Hls => "hls",
            StreamFormat::Mp4 => "mp4",
            StreamFormat::Wss => "wss",
            StreamFormat::Dash => "dash",
        }
    }

    pub fn from_extension(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "flv" => StreamFormat::Flv,
            "m3u8" => StreamFormat::Hls,
            "mp4" => StreamFormat::Mp4,
            "wss" => StreamFormat::Wss,
            "mpd" => StreamFormat::Dash,
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
            "dash" => Ok(StreamFormat::Dash),
            _ => Err(()),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MediaFormat {
    Flv,
    Ts,
    Mp4,
    Fmp4,
}

impl MediaFormat {
    pub fn as_str(&self) -> &str {
        match self {
            MediaFormat::Flv => "flv",
            MediaFormat::Ts => "ts",
            MediaFormat::Mp4 => "mp4",
            MediaFormat::Fmp4 => "fmp4",
        }
    }

    pub fn from_extension(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "flv" => MediaFormat::Flv,
            "ts" => MediaFormat::Ts,
            "mp4" => MediaFormat::Mp4,
            "fmp4" => MediaFormat::Fmp4,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_round_trips_through_every_representation() {
        assert_eq!(StreamFormat::Dash.as_str(), "dash");
        assert_eq!(StreamFormat::Dash.to_string(), "dash");
        assert_eq!("dash".parse::<StreamFormat>(), Ok(StreamFormat::Dash));
        assert_eq!("DASH".parse::<StreamFormat>(), Ok(StreamFormat::Dash));
        assert_eq!(StreamFormat::from_extension("mpd"), StreamFormat::Dash);
        assert_eq!(StreamFormat::from_extension("MPD"), StreamFormat::Dash);
        assert_eq!(
            serde_json::to_string(&StreamFormat::Dash).unwrap(),
            "\"dash\""
        );
        assert_eq!(
            serde_json::from_str::<StreamFormat>("\"dash\"").unwrap(),
            StreamFormat::Dash
        );
    }
}
