# TS Parser Crate

A Rust library for parsing MPEG Transport Stream (TS) data: PAT (Program Association Table), PMT (Program Map Table), PES headers, adaptation fields, descriptors and SCTE-35 splice information.

## Features

- **PAT Parsing**: Parse Program Association Tables to discover programs and their PMT PIDs
- **PMT Parsing**: Parse Program Map Tables to discover elementary streams and their types
- **PES and Adaptation Fields**: PES headers with PTS/DTS (`PesHeader`, `PesHeaderRef`) and adaptation fields with PCR (`AdaptationField`, `Pcr`)
- **SCTE-35**: Splice information sections (`splice_insert`, `time_signal`), auto-detected from PMT entries registered as `CUEI`
- **Descriptors and CRC-32**: Descriptor iteration (`DescriptorIterator`) and MPEG-2 section CRC validation (`validate_section_crc32`)
- **Continuity Tracking**: Per-PID continuity counter checks (`ContinuityMode`, `ContinuityStatus`)
- **Stream Type Detection**: Comprehensive support for MPEG-2, H.264, H.265, AAC, AC-3, and many other stream types
- **Error Handling**: Robust error handling with detailed error messages
- **Zero-copy Design**: Efficient parsing with minimal allocations

## Supported Stream Types

### Video Formats
- MPEG-1/2 Video
- H.264/AVC (ITU-T H.264, ISO/IEC 14496-10)
- H.265/HEVC (ITU-T H.265, ISO/IEC 23008-2)
- H.266/VVC (ITU-T H.266, ISO/IEC 23090-3)
- MPEG-4 Visual
- JPEG 2000, JPEG XS
- Chinese AVS2/AVS3
- And many more...

### Audio Formats
- MPEG-1/2 Audio
- ADTS AAC, LATM AAC
- AC-3, E-AC-3
- DTS, DTS-HD
- Dolby TrueHD, Dolby E
- And more...

## Usage

The crate is used inside this workspace as a path dependency:

```toml
[dependencies]
ts = { path = "../ts" }
bytes = "1" # Required for the zero-copy parser
```

This crate provides the zero-copy `TsParser`: a high-performance parser that uses
callbacks to process PAT/PMT data without allocations.

### Zero-Copy Parser Example (`TsParser`)

This parser is designed for high-performance scenarios. It avoids allocations by using callbacks to handle PAT and PMT data as it's discovered.

```rust
use bytes::Bytes;
use ts::{PatRef, PmtRef, TsPacketRef, TsParser};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = TsParser::new();

    // Read TS data. `Bytes` is used for efficient, zero-copy slicing.
    let ts_data = Bytes::from(std::fs::read("example.ts")?);

    // Define a callback for when a PAT is found
    let on_pat = |pat: PatRef| -> ts::Result<()> {
        println!("\nPAT found (ts_id={}):", pat.transport_stream_id);
        for program in pat.programs() {
            if program.program_number != 0 {
                println!("  Program {}: PMT PID 0x{:04X}",
                         program.program_number, program.pmt_pid);
            }
        }
        Ok(())
    };

    // Define a callback for when a PMT is found
    let on_pmt = |pmt: PmtRef| -> ts::Result<()> {
        println!("PMT found (program={}):", pmt.program_number);
        for stream_result in pmt.streams() {
            let stream = stream_result?;
            println!("  - Stream: PID=0x{:04X}, Type={:?}", stream.elementary_pid, stream.stream_type);
        }
        Ok(())
    };

    // Parse the data, triggering the callbacks as PAT/PMT sections are found.
    // The last argument is an optional per-packet callback; None skips it.
    parser.parse_packets(ts_data, on_pat, on_pmt, None::<fn(&TsPacketRef) -> ts::Result<()>>)?;

    Ok(())
}
```

### Stream Classification

```rust
use ts::StreamType;

// Check if a stream type is video or audio
let stream_type = StreamType::H264;
if stream_type.is_video() {
    println!("This is a video stream");
}

let audio_type = StreamType::AdtsAac;
if audio_type.is_audio() {
    println!("This is an audio stream");
}
```

## API Reference

### Parsers

- **`TsParser`**: A zero-copy, callback-based parser that processes TS data without allocations. Best for high-performance, low-latency applications.

### `TsParser` (Zero-Copy)

The high-performance, callback-based parser.

#### Methods

- `new() -> TsParser`: Creates a new parser instance.
- `parse_packets<F, G, H>(&mut self, data: Bytes, on_pat: F, on_pmt: G, on_packet: Option<H>) -> Result<()>`: Parses TS packets from a `Bytes` buffer and invokes callbacks when PAT or PMT sections are found.
  - `on_pat: FnMut(PatRef) -> Result<()>`: A callback invoked when a PAT is parsed.
  - `on_pmt: FnMut(PmtRef) -> Result<()>`: A callback invoked when a PMT is parsed.
  - `on_packet: Option<FnMut(&TsPacketRef) -> Result<()>>`: An optional callback invoked for every parsed packet; pass `None` to skip it.
- `parse_packets_with_scte35(...)`: Same as `parse_packets`, with an extra `on_scte35: FnMut(SpliceInfoSectionRef) -> Result<()>` callback for SCTE-35 sections.
- `reset(&mut self)`: Clears the parser's internal state.

### Data Structures

The crate provides two sets of data structures for PAT/PMT information:

- **Owned (`Pat`, `Pmt`)**: These structs hold owned data, copied from the input buffer. They are produced by `Pat::parse`/`Pmt::parse` (or `parse_with_crc` to validate the section CRC).
- **Zero-Copy (`PatRef`, `PmtRef`)**: These structs hold references to the input buffer (`Bytes`). They are passed to the callbacks in the zero-copy `TsParser`. They provide efficient iterators over programs and streams (e.g., `PatRef::programs()`, `PmtRef::streams()`).

### `StreamType`

Enum representing various stream types defined in MPEG-2 and other standards.

#### Methods

- `is_video()` - Check if this is a video stream type
- `is_audio()` - Check if this is an audio stream type

## Error Handling

The crate reports errors through `TsError`, including:

- `InvalidPacketSize` - TS packet is not 188 bytes
- `InvalidSyncByte` - Sync byte is not 0x47
- `InsufficientData` - Not enough data to parse
- `InvalidTableId` - Wrong table ID for PAT/PMT
- `InvalidSectionLength`, `Crc32Mismatch` - Malformed or corrupted PSI sections
- `ContinuityError`, `DuplicatePacket` - Continuity counter violations
- `InvalidPesStartCode`, `InvalidPtsDtsFlags` - Malformed PES headers
- `InvalidScte35` - Malformed SCTE-35 section
- `ParseError` - General parsing errors

## Testing

Run the test suite from the repository root:

```bash
cargo nextest run --locked -p ts
```

## License

This crate is licensed under MIT OR Apache-2.0. 