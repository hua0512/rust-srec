use std::time::Instant;
use ts::{PatRef, PmtRef, TsParser, ZeroCopyTsParser};

fn main() {
    println!("TS Parser Memory Usage Comparison");
    println!("=================================");

    // Create test data with multiple programs and streams
    let ts_data = create_complex_ts_data();
    println!(
        "Test data size: {} bytes ({} packets)",
        ts_data.len(),
        ts_data.len() / 188
    );

    // Benchmark original parser
    let start = Instant::now();
    let mut original_parser = TsParser::new();
    let original_result = original_parser.parse_packets(&ts_data);
    let original_duration = start.elapsed();

    match original_result {
        Ok(()) => {
            println!("\n📊 Original Parser Results:");
            println!("  Parse time: {original_duration:?}");
            if let Some(pat) = original_parser.pat() {
                println!("  PAT: {} programs", pat.programs.len());
            }
            println!("  PMTs: {} found", original_parser.pmts().len());

            // Estimate memory usage
            let mut estimated_memory = std::mem::size_of::<TsParser>();
            if let Some(pat) = original_parser.pat() {
                estimated_memory += std::mem::size_of_val(pat);
                estimated_memory += pat.programs.len() * std::mem::size_of::<ts::PatProgram>();
            }
            for pmt in original_parser.pmts().values() {
                estimated_memory += std::mem::size_of_val(pmt);
                estimated_memory += pmt.streams.len() * std::mem::size_of::<ts::PmtStream>();
                estimated_memory += pmt.program_info.len();
                for stream in &pmt.streams {
                    estimated_memory += stream.es_info.len();
                }
            }
            println!("  Estimated memory: ~{estimated_memory} bytes");
        }
        Err(e) => println!("❌ Original parser failed: {e}"),
    }

    // Benchmark zero-copy parser
    let start = Instant::now();
    let mut zero_copy_parser = ZeroCopyTsParser::new();

    let mut pat_count = 0;
    let mut pmt_count = 0;
    let mut total_streams = 0;

    let zero_copy_result = zero_copy_parser.parse_packets(
        &ts_data,
        |pat: PatRef<'_>| {
            pat_count += 1;
            println!("\n🚀 Zero-Copy PAT Found:");
            println!("  Transport Stream ID: {}", pat.transport_stream_id);
            println!("  Programs: {}", pat.program_count());
            for program in pat.programs() {
                if program.program_number != 0 {
                    println!(
                        "    Program {}: PMT PID 0x{:04X}",
                        program.program_number, program.pmt_pid
                    );
                }
            }
            Ok(())
        },
        |pmt: PmtRef<'_>| {
            pmt_count += 1;
            println!("\n🚀 Zero-Copy PMT Found (Program {}):", pmt.program_number);
            println!("  PCR PID: 0x{:04X}", pmt.pcr_pid);

            let mut video_streams = 0;
            let mut audio_streams = 0;

            for stream in pmt.streams().flatten() {
                total_streams += 1;
                if stream.stream_type.is_video() {
                    video_streams += 1;
                    println!(
                        "    Video PID 0x{:04X}: {:?}",
                        stream.elementary_pid, stream.stream_type
                    );
                } else if stream.stream_type.is_audio() {
                    audio_streams += 1;
                    println!(
                        "    Audio PID 0x{:04X}: {:?}",
                        stream.elementary_pid, stream.stream_type
                    );
                } else {
                    println!(
                        "    Other PID 0x{:04X}: {:?}",
                        stream.elementary_pid, stream.stream_type
                    );
                }
            }

            println!("  Summary: {video_streams} video, {audio_streams} audio streams");
            Ok(())
        },
    );

    let zero_copy_duration = start.elapsed();

    match zero_copy_result {
        Ok(()) => {
            println!("\n📊 Zero-Copy Parser Results:");
            println!("  Parse time: {zero_copy_duration:?}");
            println!("  PATs processed: {pat_count}");
            println!("  PMTs processed: {pmt_count}");
            println!("  Total streams: {total_streams}");

            // Zero-copy parser memory usage is minimal
            let zero_copy_memory = zero_copy_parser.estimated_memory_usage();
            println!("  Actual memory: ~{zero_copy_memory} bytes");
        }
        Err(e) => println!("❌ Zero-copy parser failed: {e}"),
    }

    // Compare performance
    println!("\n⚡ Performance Comparison:");
    if original_duration > zero_copy_duration {
        let speedup = original_duration.as_nanos() as f64 / zero_copy_duration.as_nanos() as f64;
        println!("  Zero-copy is {speedup:.2}x faster");
    } else {
        let slowdown = zero_copy_duration.as_nanos() as f64 / original_duration.as_nanos() as f64;
        println!("  Zero-copy is {slowdown:.2}x slower");
    }

    // Demonstrate streaming benefits
    println!("\n🌊 Streaming Processing Demo:");
    demonstrate_streaming(&ts_data);
}

fn demonstrate_streaming(ts_data: &[u8]) {
    println!(
        "Processing {} packets one by one without buffering...",
        ts_data.len() / 188
    );

    let mut zero_copy_parser = ZeroCopyTsParser::new();
    let mut processed_packets = 0;

    // Process packets in small chunks to simulate streaming
    for chunk in ts_data.chunks(188 * 4) {
        // 4 packets at a time
        let _ = zero_copy_parser.parse_packets(
            chunk,
            |_pat| {
                // In a real streaming scenario, you'd process the PAT immediately
                Ok(())
            },
            |_pmt| {
                // In a real streaming scenario, you'd process the PMT immediately
                Ok(())
            },
        );
        processed_packets += chunk.len() / 188;
    }

    println!("✓ Processed {processed_packets} packets with minimal memory footprint");
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

    // Add some dummy data packets
    for pid in [0x100, 0x101, 0x200, 0x201] {
        let mut data_packet = vec![0u8; 188];
        data_packet[0] = 0x47; // Sync byte
        data_packet[1] = (pid >> 8) as u8;
        data_packet[2] = (pid & 0xFF) as u8;
        data_packet[3] = 0x10; // No scrambling, payload only
        ts_data.extend_from_slice(&data_packet);
    }

    ts_data
}
