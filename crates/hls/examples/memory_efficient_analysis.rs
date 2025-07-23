use bytes::Bytes;
use hls::HlsData;
use m3u8_rs::MediaSegment;
use std::time::Instant;
use ts::StreamType;

fn main() {
    println!("HLS Memory-Efficient TS Analysis");
    println!("================================");

    // Create working TS data with multiple programs
    let ts_data = create_complex_ts_data();

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

    // Compare memory usage and performance
    compare_parsing_approaches(&hls_data);

    // Demonstrate streaming-friendly approach
    demonstrate_zero_copy_features(&hls_data);
}

fn compare_parsing_approaches(hls_data: &HlsData) {
    println!("\n🔬 Comparing Parsing Approaches:");
    println!("================================");

    // Traditional approach
    let start = Instant::now();
    let traditional_result = hls_data.parse_ts_psi_tables();
    let traditional_duration = start.elapsed();

    match traditional_result {
        Some(Ok((pat, pmts))) => {
            println!("📊 Traditional Parser:");
            println!("  Parse time: {traditional_duration:?}");
            if let Some(pat) = pat {
                println!("  PAT: {} programs", pat.programs.len());
                // Estimate memory: PAT + PMTs + descriptor data
                let mut memory_estimate = std::mem::size_of_val(&pat);
                memory_estimate += pat.programs.len() * std::mem::size_of::<ts::PatProgram>();

                for pmt in &pmts {
                    memory_estimate += std::mem::size_of_val(pmt);
                    memory_estimate += pmt.streams.len() * std::mem::size_of::<ts::PmtStream>();
                    memory_estimate += pmt.program_info.len();
                    for stream in &pmt.streams {
                        memory_estimate += stream.es_info.len();
                    }
                }
                println!("  Estimated memory: ~{memory_estimate} bytes");
            }
            println!("  PMTs: {} found", pmts.len());
        }
        Some(Err(e)) => println!("❌ Traditional parser failed: {e}"),
        None => println!("❌ Not a TS segment"),
    }

    // Zero-copy approach
    let start = Instant::now();
    let zero_copy_result = hls_data.parse_ts_psi_tables_zero_copy();
    let zero_copy_duration = start.elapsed();

    match zero_copy_result {
        Some(Ok(stream_info)) => {
            println!("🚀 Zero-Copy Parser:");
            println!("  Parse time: {zero_copy_duration:?}");
            println!("  Transport Stream ID: {}", stream_info.transport_stream_id);
            println!("  Programs: {} found", stream_info.programs.len());

            // Zero-copy memory is just the lightweight structs
            let memory_usage = std::mem::size_of_val(&stream_info)
                + stream_info.programs.len() * std::mem::size_of::<hls::ProgramInfo>();
            println!("  Actual memory: ~{memory_usage} bytes");

            for program in &stream_info.programs {
                println!(
                    "    Program {}: {} video, {} audio, {} other streams",
                    program.program_number,
                    program.video_streams.len(),
                    program.audio_streams.len(),
                    program.other_streams.len()
                );
            }
        }
        Some(Err(e)) => println!("❌ Zero-copy parser failed: {e}"),
        None => println!("❌ Not a TS segment"),
    }

    // Performance comparison
    if traditional_duration > zero_copy_duration {
        let speedup = traditional_duration.as_nanos() as f64 / zero_copy_duration.as_nanos() as f64;
        println!("⚡ Zero-copy is {speedup:.2}x faster");
    } else {
        let slowdown =
            zero_copy_duration.as_nanos() as f64 / traditional_duration.as_nanos() as f64;
        println!("⚡ Zero-copy is {slowdown:.2}x slower (but uses less memory)");
    }
}

fn demonstrate_zero_copy_features(hls_data: &HlsData) {
    println!("\n🌟 Zero-Copy Features Demo:");
    println!("==========================");

    // Stream summary with minimal memory usage
    if let Some(summary) = hls_data.get_stream_summary_zero_copy() {
        println!("📊 Stream summary (zero-copy): {summary}");
    }

    // Check for specific stream types efficiently
    if hls_data.ts_contains_stream_type_zero_copy(StreamType::H264) {
        println!("✓ Contains H.264 video (detected with zero-copy)");
    }
    if hls_data.ts_contains_stream_type_zero_copy(StreamType::H265) {
        println!("✓ Contains H.265 video (detected with zero-copy)");
    }
    if hls_data.ts_contains_stream_type_zero_copy(StreamType::AdtsAac) {
        println!("✓ Contains AAC audio (detected with zero-copy)");
    }
    if hls_data.ts_contains_stream_type_zero_copy(StreamType::Ac3) {
        println!("✓ Contains AC-3 audio (detected with zero-copy)");
    }

    // Get stream lists efficiently
    if let Some(Ok(video_streams)) = hls_data.get_ts_video_streams_zero_copy() {
        println!("🎥 Video streams (zero-copy):");
        for (pid, stream_type) in video_streams {
            println!("   PID 0x{pid:04X}: {stream_type:?}");
        }
    }

    println!("\n💡 Benefits of Zero-Copy Approach:");
    println!("  • No allocation of descriptor data");
    println!("  • Minimal memory footprint");
    println!("  • Suitable for high-throughput streaming");
    println!("  • Lower GC pressure in long-running applications");
    println!("  • Better cache locality when processing many segments");
}

fn create_complex_ts_data() -> Vec<u8> {
    let mut ts_data = Vec::new();

    // Create PAT with multiple programs
    let mut pat_packet = vec![0u8; 188];
    pat_packet[0] = 0x47; // Sync byte
    pat_packet[1] = 0x40; // PUSI set, PID = 0 (PAT)
    pat_packet[2] = 0x00;
    pat_packet[3] = 0x10; // No scrambling, payload only

    // PAT with 2 programs
    pat_packet[4] = 0x00; // Pointer field
    pat_packet[5] = 0x00; // Table ID (PAT)
    pat_packet[6] = 0x80; // Section syntax indicator
    pat_packet[7] = 0x11; // Section length (17 bytes for 2 programs)
    pat_packet[8] = 0x00; // Transport stream ID high
    pat_packet[9] = 0x01; // Transport stream ID low
    pat_packet[10] = 0x01; // Version 0 + current/next = 1
    pat_packet[11] = 0x00; // Section number
    pat_packet[12] = 0x00; // Last section number
    // Program 1
    pat_packet[13] = 0x00;
    pat_packet[14] = 0x01; // Program number 1
    pat_packet[15] = 0xE1;
    pat_packet[16] = 0x00; // PMT PID 0x100
    // Program 2
    pat_packet[17] = 0x00;
    pat_packet[18] = 0x02; // Program number 2
    pat_packet[19] = 0xE2;
    pat_packet[20] = 0x00; // PMT PID 0x200
    // CRC32
    pat_packet[21] = 0x00;
    pat_packet[22] = 0x00;
    pat_packet[23] = 0x00;
    pat_packet[24] = 0x00;

    // PMT for program 1 (H.264 + AAC)
    let mut pmt1_packet = vec![0u8; 188];
    pmt1_packet[0] = 0x47; // Sync byte
    pmt1_packet[1] = 0x41; // PUSI set, PID = 0x100
    pmt1_packet[2] = 0x00;
    pmt1_packet[3] = 0x10; // No scrambling, payload only

    pmt1_packet[4] = 0x00; // Pointer field
    pmt1_packet[5] = 0x02; // Table ID (PMT)
    pmt1_packet[6] = 0x80; // Section syntax indicator
    pmt1_packet[7] = 0x17; // Section length (23 bytes for 2 streams)
    pmt1_packet[8] = 0x00;
    pmt1_packet[9] = 0x01; // Program number 1
    pmt1_packet[10] = 0x01; // Version 0 + current/next = 1
    pmt1_packet[11] = 0x00; // Section number
    pmt1_packet[12] = 0x00; // Last section number
    pmt1_packet[13] = 0xE1;
    pmt1_packet[14] = 0x00; // PCR PID 0x100
    pmt1_packet[15] = 0x00;
    pmt1_packet[16] = 0x00; // Program info length
    // Video stream (H.264)
    pmt1_packet[17] = 0x1B; // Stream type H.264
    pmt1_packet[18] = 0xE1;
    pmt1_packet[19] = 0x00; // Elementary PID 0x100
    pmt1_packet[20] = 0x00;
    pmt1_packet[21] = 0x00; // ES info length
    // Audio stream (AAC)
    pmt1_packet[22] = 0x0F; // Stream type ADTS AAC
    pmt1_packet[23] = 0xE1;
    pmt1_packet[24] = 0x01; // Elementary PID 0x101
    pmt1_packet[25] = 0x00;
    pmt1_packet[26] = 0x00; // ES info length
    // CRC32
    pmt1_packet[27] = 0x00;
    pmt1_packet[28] = 0x00;
    pmt1_packet[29] = 0x00;
    pmt1_packet[30] = 0x00;

    // PMT for program 2 (H.265 + AC-3)
    let mut pmt2_packet = vec![0u8; 188];
    pmt2_packet[0] = 0x47; // Sync byte
    pmt2_packet[1] = 0x42; // PUSI set, PID = 0x200
    pmt2_packet[2] = 0x00;
    pmt2_packet[3] = 0x10; // No scrambling, payload only

    pmt2_packet[4] = 0x00; // Pointer field
    pmt2_packet[5] = 0x02; // Table ID (PMT)
    pmt2_packet[6] = 0x80; // Section syntax indicator
    pmt2_packet[7] = 0x17; // Section length
    pmt2_packet[8] = 0x00;
    pmt2_packet[9] = 0x02; // Program number 2
    pmt2_packet[10] = 0x01; // Version 0 + current/next = 1
    pmt2_packet[11] = 0x00; // Section number
    pmt2_packet[12] = 0x00; // Last section number
    pmt2_packet[13] = 0xE2;
    pmt2_packet[14] = 0x00; // PCR PID 0x200
    pmt2_packet[15] = 0x00;
    pmt2_packet[16] = 0x00; // Program info length
    // Video stream (H.265)
    pmt2_packet[17] = 0x24; // Stream type H.265
    pmt2_packet[18] = 0xE2;
    pmt2_packet[19] = 0x00; // Elementary PID 0x200
    pmt2_packet[20] = 0x00;
    pmt2_packet[21] = 0x00; // ES info length
    // Audio stream (AC-3)
    pmt2_packet[22] = 0x81; // Stream type AC-3
    pmt2_packet[23] = 0xE2;
    pmt2_packet[24] = 0x01; // Elementary PID 0x201
    pmt2_packet[25] = 0x00;
    pmt2_packet[26] = 0x00; // ES info length
    // CRC32
    pmt2_packet[27] = 0x00;
    pmt2_packet[28] = 0x00;
    pmt2_packet[29] = 0x00;
    pmt2_packet[30] = 0x00;

    ts_data.extend_from_slice(&pat_packet);
    ts_data.extend_from_slice(&pmt1_packet);
    ts_data.extend_from_slice(&pmt2_packet);

    ts_data
}
