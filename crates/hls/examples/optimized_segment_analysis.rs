use bytes::Bytes;
use hls::HlsData;
use m3u8_rs::MediaSegment;
use std::time::Instant;

fn main() {
    println!("Optimized HLS Segment Analysis");
    println!("==============================");
    println!("Demonstrating improved segment logic with zero-copy parsing by default\n");

    // Create test segments with different content
    let segments = create_test_segments();

    println!("📊 Analyzing {} test segments...\n", segments.len());

    // Benchmark the optimized segment analysis
    let start = Instant::now();
    analyze_segments(&segments);
    let duration = start.elapsed();

    println!("\n⚡ Performance:");
    println!("  Total analysis time: {duration:?}");
    println!(
        "  Average per segment: {:?}",
        duration / segments.len() as u32
    );
    println!(
        "  Segments per second: {:.1}",
        segments.len() as f64 / duration.as_secs_f64()
    );
}

fn analyze_segments(segments: &[HlsData]) {
    for (i, segment) in segments.iter().enumerate() {
        println!("🎬 Segment {}:", i + 1);

        // Quick segment type check
        println!(
            "  Type: {:?} ({} bytes)",
            segment.segment_type(),
            segment.size()
        );

        if segment.is_ts() {
            // Use the new optimized segment logic
            analyze_ts_segment(segment, i + 1);
        } else if segment.is_mp4() {
            println!("  MP4 segment analysis not implemented in this demo");
        }

        println!();
    }
}

fn analyze_ts_segment(segment: &HlsData, segment_num: usize) {
    // Test the optimized PSI detection
    let has_psi = segment.ts_has_psi_tables();
    println!("  Contains PSI tables: {}", if has_psi { "✓" } else { "✗" });

    if !has_psi {
        println!("  No PSI tables found - cannot analyze streams");
        return;
    }

    // Use the new quick check methods (all zero-copy)
    println!("  Quick Checks (zero-copy):");
    println!(
        "    Has video: {}",
        if segment.has_video_streams() {
            "✓"
        } else {
            "✗"
        }
    );
    println!(
        "    Has audio: {}",
        if segment.has_audio_streams() {
            "✓"
        } else {
            "✗"
        }
    );

    // Codec-specific checks
    if segment.has_h264_video() {
        println!("    H.264 video: ✓");
    }
    if segment.has_h265_video() {
        println!("    H.265 video: ✓");
    }
    if segment.has_aac_audio() {
        println!("    AAC audio: ✓");
    }
    if segment.has_ac3_audio() {
        println!("    AC-3 audio: ✓");
    }

    // Get comprehensive stream profile
    if let Some(profile) = segment.get_stream_profile() {
        println!("  Stream Profile:");
        println!(
            "    Complete stream: {}",
            if profile.is_complete() { "✓" } else { "✗" }
        );
        println!("    Codec info: {}", profile.codec_description());
        println!("    Summary: {}", profile.summary);

        // Demonstrate profile analysis
        if profile.is_complete() {
            println!("    🎯 This is a complete multimedia segment");
        } else if profile.has_video && !profile.has_audio {
            println!("    📹 Video-only segment");
        } else if profile.has_audio && !profile.has_video {
            println!("    🔊 Audio-only segment");
        }
    }

    // Show detailed stream information if needed
    if segment_num <= 2 {
        show_detailed_streams(segment);
    }
}

fn show_detailed_streams(segment: &HlsData) {
    println!("  Detailed Stream Info (zero-copy):");

    // Get all streams efficiently
    if let Some(Ok(streams)) = segment.get_ts_all_streams() {
        for (pid, stream_type) in streams {
            let category = if stream_type.is_video() {
                "Video"
            } else if stream_type.is_audio() {
                "Audio"
            } else {
                "Other"
            };
            println!("    PID 0x{pid:04X}: {category} ({stream_type:?})");
        }
    }

    // Alternative: get video and audio separately
    if let Some(Ok(video_streams)) = segment.get_ts_video_streams() {
        if !video_streams.is_empty() {
            println!("  Video streams: {}", video_streams.len());
        }
    }

    if let Some(Ok(audio_streams)) = segment.get_ts_audio_streams() {
        if !audio_streams.is_empty() {
            println!("  Audio streams: {}", audio_streams.len());
        }
    }
}

fn create_test_segments() -> Vec<HlsData> {
    vec![
        // Segment 1: H.264 + AAC
        create_h264_aac_segment(),
        // Segment 2: H.265 + AC-3
        create_h265_ac3_segment(),
        // Segment 3: Multiple programs
        create_multi_program_segment(),
        // Segment 4: End marker
        HlsData::end_marker(),
    ]
}

fn create_h264_aac_segment() -> HlsData {
    let ts_data = create_ts_data_h264_aac();
    let segment = create_media_segment("segment001.ts");
    HlsData::ts(segment, Bytes::from(ts_data))
}

fn create_h265_ac3_segment() -> HlsData {
    let ts_data = create_ts_data_h265_ac3();
    let segment = create_media_segment("segment002.ts");
    HlsData::ts(segment, Bytes::from(ts_data))
}

fn create_multi_program_segment() -> HlsData {
    let ts_data = create_ts_data_multi_program();
    let segment = create_media_segment("segment003.ts");
    HlsData::ts(segment, Bytes::from(ts_data))
}

fn create_media_segment(uri: &str) -> MediaSegment {
    MediaSegment {
        uri: uri.to_string(),
        duration: 6.0,
        title: None,
        byte_range: None,
        discontinuity: false,
        key: None,
        map: None,
        program_date_time: None,
        daterange: None,
        unknown_tags: vec![],
    }
}

fn create_ts_data_h264_aac() -> Vec<u8> {
    let mut ts_data = Vec::new();

    // PAT packet
    let mut pat_packet = vec![0u8; 188];
    pat_packet[0] = 0x47; // Sync byte
    pat_packet[1] = 0x40; // PUSI set, PID = 0
    pat_packet[2] = 0x00;
    pat_packet[3] = 0x10; // No scrambling, payload only

    // Simple PAT
    pat_packet[4] = 0x00; // Pointer field
    pat_packet[5] = 0x00; // Table ID (PAT)
    pat_packet[6] = 0x80; // Section syntax indicator
    pat_packet[7] = 0x0D; // Section length
    pat_packet[8] = 0x00;
    pat_packet[9] = 0x01; // Transport stream ID
    pat_packet[10] = 0x01; // Version + current/next
    pat_packet[11] = 0x00;
    pat_packet[12] = 0x00; // Section numbers
    // Program entry
    pat_packet[13] = 0x00;
    pat_packet[14] = 0x01; // Program number 1
    pat_packet[15] = 0xE1;
    pat_packet[16] = 0x00; // PMT PID 0x100

    // PMT packet (H.264 + AAC)
    let mut pmt_packet = vec![0u8; 188];
    pmt_packet[0] = 0x47; // Sync byte
    pmt_packet[1] = 0x41; // PUSI set, PID = 0x100
    pmt_packet[2] = 0x00;
    pmt_packet[3] = 0x10; // No scrambling, payload only

    pmt_packet[4] = 0x00; // Pointer field
    pmt_packet[5] = 0x02; // Table ID (PMT)
    pmt_packet[6] = 0x80; // Section syntax indicator
    pmt_packet[7] = 0x17; // Section length (23 bytes for 2 streams)
    pmt_packet[8] = 0x00;
    pmt_packet[9] = 0x01; // Program number 1
    pmt_packet[10] = 0x01; // Version + current/next
    pmt_packet[11] = 0x00;
    pmt_packet[12] = 0x00; // Section numbers
    pmt_packet[13] = 0xE1;
    pmt_packet[14] = 0x00; // PCR PID 0x100
    pmt_packet[15] = 0x00;
    pmt_packet[16] = 0x00; // Program info length
    // Video stream (H.264)
    pmt_packet[17] = 0x1B; // H.264
    pmt_packet[18] = 0xE1;
    pmt_packet[19] = 0x00; // PID 0x100
    pmt_packet[20] = 0x00;
    pmt_packet[21] = 0x00; // ES info length
    // Audio stream (AAC)
    pmt_packet[22] = 0x0F; // ADTS AAC
    pmt_packet[23] = 0xE1;
    pmt_packet[24] = 0x01; // PID 0x101
    pmt_packet[25] = 0x00;
    pmt_packet[26] = 0x00; // ES info length

    ts_data.extend_from_slice(&pat_packet);
    ts_data.extend_from_slice(&pmt_packet);
    ts_data
}

fn create_ts_data_h265_ac3() -> Vec<u8> {
    let mut ts_data = Vec::new();

    // PAT packet
    let mut pat_packet = vec![0u8; 188];
    pat_packet[0] = 0x47;
    pat_packet[1] = 0x40;
    pat_packet[2] = 0x00;
    pat_packet[3] = 0x10;
    pat_packet[4] = 0x00;
    pat_packet[5] = 0x00;
    pat_packet[6] = 0x80;
    pat_packet[7] = 0x0D;
    pat_packet[8] = 0x00;
    pat_packet[9] = 0x01;
    pat_packet[10] = 0x01;
    pat_packet[11] = 0x00;
    pat_packet[12] = 0x00;
    pat_packet[13] = 0x00;
    pat_packet[14] = 0x01;
    pat_packet[15] = 0xE1;
    pat_packet[16] = 0x00;

    // PMT packet (H.265 + AC-3)
    let mut pmt_packet = vec![0u8; 188];
    pmt_packet[0] = 0x47;
    pmt_packet[1] = 0x41;
    pmt_packet[2] = 0x00;
    pmt_packet[3] = 0x10;
    pmt_packet[4] = 0x00;
    pmt_packet[5] = 0x02;
    pmt_packet[6] = 0x80;
    pmt_packet[7] = 0x17;
    pmt_packet[8] = 0x00;
    pmt_packet[9] = 0x01;
    pmt_packet[10] = 0x01;
    pmt_packet[11] = 0x00;
    pmt_packet[12] = 0x00;
    pmt_packet[13] = 0xE1;
    pmt_packet[14] = 0x00;
    pmt_packet[15] = 0x00;
    pmt_packet[16] = 0x00;
    // Video stream (H.265)
    pmt_packet[17] = 0x24; // H.265
    pmt_packet[18] = 0xE1;
    pmt_packet[19] = 0x00; // PID 0x100
    pmt_packet[20] = 0x00;
    pmt_packet[21] = 0x00;
    // Audio stream (AC-3)
    pmt_packet[22] = 0x81; // AC-3
    pmt_packet[23] = 0xE1;
    pmt_packet[24] = 0x01; // PID 0x101
    pmt_packet[25] = 0x00;
    pmt_packet[26] = 0x00;

    ts_data.extend_from_slice(&pat_packet);
    ts_data.extend_from_slice(&pmt_packet);
    ts_data
}

fn create_ts_data_multi_program() -> Vec<u8> {
    let mut ts_data = Vec::new();

    // PAT with 2 programs
    let mut pat_packet = vec![0u8; 188];
    pat_packet[0] = 0x47;
    pat_packet[1] = 0x40;
    pat_packet[2] = 0x00;
    pat_packet[3] = 0x10;
    pat_packet[4] = 0x00;
    pat_packet[5] = 0x00;
    pat_packet[6] = 0x80;
    pat_packet[7] = 0x11;
    pat_packet[8] = 0x00;
    pat_packet[9] = 0x01;
    pat_packet[10] = 0x01;
    pat_packet[11] = 0x00;
    pat_packet[12] = 0x00;
    // Program 1
    pat_packet[13] = 0x00;
    pat_packet[14] = 0x01;
    pat_packet[15] = 0xE1;
    pat_packet[16] = 0x00;
    // Program 2
    pat_packet[17] = 0x00;
    pat_packet[18] = 0x02;
    pat_packet[19] = 0xE2;
    pat_packet[20] = 0x00;

    // PMT 1 (H.264 + AAC)
    let mut pmt1_packet = vec![0u8; 188];
    pmt1_packet[0] = 0x47;
    pmt1_packet[1] = 0x41;
    pmt1_packet[2] = 0x00;
    pmt1_packet[3] = 0x10;
    pmt1_packet[4] = 0x00;
    pmt1_packet[5] = 0x02;
    pmt1_packet[6] = 0x80;
    pmt1_packet[7] = 0x17;
    pmt1_packet[8] = 0x00;
    pmt1_packet[9] = 0x01;
    pmt1_packet[10] = 0x01;
    pmt1_packet[11] = 0x00;
    pmt1_packet[12] = 0x00;
    pmt1_packet[13] = 0xE1;
    pmt1_packet[14] = 0x00;
    pmt1_packet[15] = 0x00;
    pmt1_packet[16] = 0x00;
    pmt1_packet[17] = 0x1B; // H.264
    pmt1_packet[18] = 0xE1;
    pmt1_packet[19] = 0x00;
    pmt1_packet[20] = 0x00;
    pmt1_packet[21] = 0x00;
    pmt1_packet[22] = 0x0F; // AAC
    pmt1_packet[23] = 0xE1;
    pmt1_packet[24] = 0x01;
    pmt1_packet[25] = 0x00;
    pmt1_packet[26] = 0x00;

    // PMT 2 (H.265 + AC-3)
    let mut pmt2_packet = vec![0u8; 188];
    pmt2_packet[0] = 0x47;
    pmt2_packet[1] = 0x42;
    pmt2_packet[2] = 0x00;
    pmt2_packet[3] = 0x10;
    pmt2_packet[4] = 0x00;
    pmt2_packet[5] = 0x02;
    pmt2_packet[6] = 0x80;
    pmt2_packet[7] = 0x17;
    pmt2_packet[8] = 0x00;
    pmt2_packet[9] = 0x02;
    pmt2_packet[10] = 0x01;
    pmt2_packet[11] = 0x00;
    pmt2_packet[12] = 0x00;
    pmt2_packet[13] = 0xE2;
    pmt2_packet[14] = 0x00;
    pmt2_packet[15] = 0x00;
    pmt2_packet[16] = 0x00;
    pmt2_packet[17] = 0x24; // H.265
    pmt2_packet[18] = 0xE2;
    pmt2_packet[19] = 0x00;
    pmt2_packet[20] = 0x00;
    pmt2_packet[21] = 0x00;
    pmt2_packet[22] = 0x81; // AC-3
    pmt2_packet[23] = 0xE2;
    pmt2_packet[24] = 0x01;
    pmt2_packet[25] = 0x00;
    pmt2_packet[26] = 0x00;

    ts_data.extend_from_slice(&pat_packet);
    ts_data.extend_from_slice(&pmt1_packet);
    ts_data.extend_from_slice(&pmt2_packet);
    ts_data
}
