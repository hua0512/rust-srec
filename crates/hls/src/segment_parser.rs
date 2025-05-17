/// Media format types that can be detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFormat {
    /// MPEG-2 Transport Stream
    TransportStream,
    /// MP4 Fragment (fMP4/CMAF)
    FragmentedMp4,
    /// WebVTT subtitles
    WebVtt,
    /// Unknown format
    Unknown,
}

/// Detect the format of a media segment from its contents
#[inline]
pub fn detect_format(data: &[u8]) -> MediaFormat {
    if data.len() < 4 {
        return MediaFormat::Unknown;
    }

    // Check for TS sync byte pattern (0x47 every 188 bytes)
    if data[0] == 0x47 && data.len() >= 188 && data[188] == 0x47 {
        return MediaFormat::TransportStream;
    }

    // Check for MP4 box signature
    // Look for ftyp, styp, moof, or moov boxes which indicate MP4
    if data.len() >= 8 {
        let box_type = &data[4..8];
        if box_type == b"ftyp" || box_type == b"styp" || box_type == b"moof" || box_type == b"moov"
        {
            return MediaFormat::FragmentedMp4;
        }
    }

    // Check for WebVTT signature
    if data.len() >= 6 && &data[0..6] == b"WEBVTT" {
        return MediaFormat::WebVtt;
    }

    MediaFormat::Unknown
}

/// Determine if a segment is an initialization segment
#[inline]
pub fn is_init_segment(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }

    // Look for moov box which indicates initialization segment
    for i in 0..data.len() - 8 {
        if &data[i + 4..i + 8] == b"moov" {
            return true;
        }
    }

    // Also check for explicit init segment markers
    let path_lower = String::from_utf8_lossy(&data[0..data.len().min(64)]).to_lowercase();
    path_lower.contains("init") || path_lower.contains("initialize")
}

/// Check if a transport stream segment contains a PAT (Program Association Table)
#[inline]
pub fn ts_has_pat(data: &[u8]) -> bool {
    if data.len() < 188 || data[0] != 0x47 {
        return false;
    }

    // A PAT packet has PID 0x0000
    let pid = ((data[1] & 0x1F) as u16) << 8 | (data[2] as u16);
    pid == 0
}
