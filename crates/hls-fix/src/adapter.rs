use bytes::Bytes;
use hls::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData, TsSegmentData};
use m3u8_rs::MediaSegment;
use std::path::Path;

/// Detect format of a segment and create appropriate HlsData
pub fn detect_and_create_hls_data(
    segment: MediaSegment,
    data: Bytes,
    url: Option<&str>,
) -> HlsData {
    // Try to detect from URL first if available
    if let Some(url_str) = url {
        let path = Path::new(url_str);
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();

            // Check for known extensions
            match ext_str.as_str() {
                "ts" => return HlsData::ts(segment, data),
                "m4s" => return HlsData::mp4_segment(segment, data),
                "mp4" | "cmfv" => {
                    // Check filename for init segment indicators
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase();
                    if filename.contains("init") || filename.contains("header") {
                        return HlsData::mp4_init(segment, data);
                    } else {
                        return HlsData::mp4_segment(segment, data);
                    }
                }
                _ => {} // Fall through to content-based detection
            }
        }
    }

    // Content-based detection
    if data.len() >= 4 {
        // Check for TS sync byte pattern
        if data[0] == 0x47 && data.len() >= 188 && data[188] == 0x47 {
            return HlsData::TsData(TsSegmentData { segment, data });
        }

        // Check for MP4 box signatures
        if data.len() >= 8 {
            let box_type = &data[4..8];

            // Check for MP4 init segment indicators (moov box)
            for i in 0..data.len().min(1024) - 8 {
                if &data[i + 4..i + 8] == b"moov" {
                    return HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
                        segment,
                        data,
                    }));
                }
            }

            // Check for common MP4 segment indicators
            if box_type == b"ftyp" || box_type == b"styp" || box_type == b"moof" {
                return HlsData::M4sData(M4sData::Segment(M4sSegmentData { segment, data }));
            }
        }
    }

    // Default to TS if we can't determine the type
    HlsData::TsData(TsSegmentData { segment, data })
}
