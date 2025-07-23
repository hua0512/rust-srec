use bytes::Bytes;
use hls::HlsData;
use m3u8_rs::MediaSegment;

fn main() {
    println!("Debug HLS TS Integration");
    println!("=======================");

    // Create the exact same working TS data as the debug_ts example
    let ts_data = create_working_ts_data();

    // Create a media segment metadata
    let media_segment = MediaSegment {
        uri: "segment001.ts".to_string(),
        duration: 6.0,
        title: None,
        byte_range: None,
        discontinuity: false,
        key: None,
        map: None,
        program_date_time: None,
        daterange: None,
        unknown_tags: vec![],
    };

    // Create HLS data for the TS segment
    let hls_data = HlsData::ts(media_segment, Bytes::from(ts_data));

    println!("✓ Created HLS TS segment data");
    println!("  Segment size: {} bytes", hls_data.size());
    println!("  Is TS: {}", hls_data.is_ts());

    // Try to parse PSI tables directly from HLS data
    if let Some(result) = hls_data.parse_ts_psi_tables() {
        match result {
            Ok((pat, pmts)) => {
                println!("✓ Successfully parsed PSI tables via HLS");
                if let Some(pat) = pat {
                    println!(
                        "  PAT: Transport Stream ID {}, {} programs",
                        pat.transport_stream_id,
                        pat.programs.len()
                    );
                }
                println!("  PMTs: {} found", pmts.len());
                for pmt in pmts {
                    println!(
                        "    Program {}: {} streams",
                        pmt.program_number,
                        pmt.streams.len()
                    );
                }
            }
            Err(e) => {
                println!("❌ Failed to parse PSI tables via HLS: {e}");
            }
        }
    } else {
        println!("❌ Not a TS segment");
    }

    // Test individual stream methods
    if let Some(Ok(video_streams)) = hls_data.get_ts_video_streams() {
        println!("✓ Video streams: {} found", video_streams.len());
    } else {
        println!("❌ Failed to get video streams");
    }
}

fn create_working_ts_data() -> Vec<u8> {
    // Use the exact test data format that works in debug_ts
    let mut ts_data = Vec::new();

    // PAT packet (188 bytes)
    let mut pat_packet = vec![0u8; 188];
    pat_packet[0] = 0x47; // Sync byte
    pat_packet[1] = 0x40; // PUSI set, PID = 0 (PAT)
    pat_packet[2] = 0x00;
    pat_packet[3] = 0x10; // No scrambling, payload only, continuity = 0

    // Simple PAT payload (based on ts crate test)
    pat_packet[4] = 0x00; // Pointer field
    pat_packet[5] = 0x00; // Table ID (PAT)
    pat_packet[6] = 0x80; // Section syntax indicator + reserved + section length high
    pat_packet[7] = 0x0D; // Section length low (13 bytes)
    pat_packet[8] = 0x00; // Transport stream ID high
    pat_packet[9] = 0x01; // Transport stream ID low
    pat_packet[10] = 0x01; // Version 0 + current/next = 1
    pat_packet[11] = 0x00; // Section number
    pat_packet[12] = 0x00; // Last section number
    // Program entry
    pat_packet[13] = 0x00; // Program number high
    pat_packet[14] = 0x01; // Program number low (1)
    pat_packet[15] = 0xE1; // PMT PID high (0x100)
    pat_packet[16] = 0x00; // PMT PID low
    // CRC32 (4 bytes) - leaving as zeros for now
    pat_packet[17] = 0x00;
    pat_packet[18] = 0x00;
    pat_packet[19] = 0x00;
    pat_packet[20] = 0x00;

    // PMT packet (188 bytes)
    let mut pmt_packet = vec![0u8; 188];
    pmt_packet[0] = 0x47; // Sync byte
    pmt_packet[1] = 0x41; // PUSI set, PID = 0x100 (PMT)
    pmt_packet[2] = 0x00;
    pmt_packet[3] = 0x10; // No scrambling, payload only, continuity = 0

    // Simple PMT payload
    pmt_packet[4] = 0x00; // Pointer field
    pmt_packet[5] = 0x02; // Table ID (PMT)
    pmt_packet[6] = 0x80; // Section syntax indicator + reserved + section length high
    pmt_packet[7] = 0x12; // Section length low (18 bytes)
    pmt_packet[8] = 0x00; // Program number high
    pmt_packet[9] = 0x01; // Program number low
    pmt_packet[10] = 0x01; // Version 0 + current/next = 1
    pmt_packet[11] = 0x00; // Section number
    pmt_packet[12] = 0x00; // Last section number
    pmt_packet[13] = 0xE1; // PCR PID high (0x100)
    pmt_packet[14] = 0x00; // PCR PID low
    pmt_packet[15] = 0x00; // Program info length high
    pmt_packet[16] = 0x00; // Program info length low
    // Elementary stream
    pmt_packet[17] = 0x1B; // Stream type (H.264)
    pmt_packet[18] = 0xE1; // Elementary PID high (0x100)
    pmt_packet[19] = 0x00; // Elementary PID low
    pmt_packet[20] = 0x00; // ES info length high
    pmt_packet[21] = 0x00; // ES info length low
    // CRC32 (4 bytes)
    pmt_packet[22] = 0x00;
    pmt_packet[23] = 0x00;
    pmt_packet[24] = 0x00;
    pmt_packet[25] = 0x00;

    ts_data.extend_from_slice(&pat_packet);
    ts_data.extend_from_slice(&pmt_packet);

    ts_data
}
