use std::fmt::Display;
use std::str::FromStr;

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
            _ => Err(()),
        }
    }
}