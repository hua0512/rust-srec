# TS Parser Crate

A Rust library for parsing MPEG Transport Stream (TS) PAT (Program Association Table) and PMT (Program Map Table) data.

## Features

- **PAT Parsing**: Parse Program Association Tables to discover programs and their PMT PIDs
- **PMT Parsing**: Parse Program Map Tables to discover elementary streams and their types
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

Add this to your `Cargo.toml`:

```toml
[dependencies]
ts = "0.1.0"
bytes = "1.0" # Required for the zero-copy parser
```

This crate provides two main parsers to suit different needs:
- `OwnedTsParser`: A simple-to-use parser that owns and stores the parsed PAT/PMT data.
- `TsParser`: A high-performance, zero-copy parser that uses callbacks to process data without allocations.

### Owned Parser Example (`OwnedTsParser`)

This parser is ideal when you want to parse a chunk of data and inspect the PAT/PMT information afterward.

```rust
use ts::{OwnedTsParser, StreamType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = OwnedTsParser::new();

    // Read TS data from a file or network buffer
    let ts_data = std::fs::read("example.ts")?;

    // Parse the TS packets. The parser will store the PAT and PMTs internally.
    parser.parse_packets(&ts_data)?;

    // Access PAT information
    if let Some(pat) = parser.pat() {
        println!("Transport Stream ID: {}", pat.transport_stream_id);
        for program in &pat.programs {
            if program.program_number != 0 {
                println!("  Program {}: PMT PID 0x{:04X}",
                         program.program_number, program.pmt_pid);
            }
        }
    }

    // Access PMT information
    for (program_num, pmt) in parser.pmts() {
        println!("Program {} streams:", program_num);
        for stream in &pmt.streams {
            println!("  - PID: 0x{:04X}, Type: {:?}", stream.elementary_pid, stream.stream_type);
        }
    }

    Ok(())
}```

### Zero-Copy Parser Example (`TsParser`)

This parser is designed for high-performance scenarios. It avoids allocations by using callbacks to handle PAT and PMT data as it's discovered.

```rust
use bytes::Bytes;
use ts::{TsParser, PatRef, PmtRef, StreamType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = TsParser::new();

    // Read TS data. `Bytes` is used for efficient, zero-copy slicing.
    let ts_data = Bytes::from(std::fs::read("example.ts")?);

    // Define a callback for when a PAT is found
    let on_pat = |pat: PatRef| {
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
    let on_pmt = |pmt: PmtRef| {
        println!("PMT found (program={}):", pmt.program_number);
        for stream_result in pmt.streams() {
            let stream = stream_result?;
            println!("  - Stream: PID=0x{:04X}, Type={:?}", stream.elementary_pid, stream.stream_type);
        }
        Ok(())
    };

    // Parse the data, triggering the callbacks as PAT/PMT sections are found
    parser.parse_packets(ts_data, on_pat, on_pmt)?;

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

- **`OwnedTsParser`**: An owned parser that copies and manages PAT/PMT data internally. Best for when you need to store the parsed tables for later access.
- **`TsParser`**: A zero-copy, callback-based parser that processes TS data without allocations. Best for high-performance, low-latency applications.

### `OwnedTsParser`

The stateful parser that stores parsed tables.

#### Methods

- `new() -> OwnedTsParser`: Creates a new parser instance.
- `parse_packets(&mut self, data: &[u8]) -> Result<()>`: Parses TS packets from a byte slice. PATs and PMTs are stored internally.
- `pat(&self) -> Option<&Pat>`: Returns a reference to the parsed Program Association Table.
- `pmts(&self) -> &HashMap<u16, Pmt>`: Returns a map of all parsed Program Map Tables, keyed by program number.
- `pmt(&self, program_number: u16) -> Option<&Pmt>`: Returns a reference to a specific PMT for a given program number.
- `reset(&mut self)`: Clears all internal state (PAT, PMTs, etc.).

### `TsParser` (Zero-Copy)

The high-performance, callback-based parser.

#### Methods

- `new() -> TsParser`: Creates a new parser instance.
- `parse_packets<F, G>(&mut self, data: Bytes, on_pat: F, on_pmt: G) -> Result<()>`: Parses TS packets from a `Bytes` buffer and invokes callbacks when PAT or PMT sections are found.
  - `on_pat: FnMut(PatRef) -> Result<()>`: A callback invoked when a PAT is parsed.
  - `on_pmt: FnMut(PmtRef) -> Result<()>`: A callback invoked when a PMT is parsed.
- `reset(&mut self)`: Clears the parser's internal state.

### Data Structures

The crate provides two sets of data structures for PAT/PMT information:

- **Owned (`Pat`, `Pmt`)**: These structs hold owned data, copied from the input buffer. They are returned by the `OwnedTsParser`.
- **Zero-Copy (`PatRef`, `PmtRef`)**: These structs hold references to the input buffer (`Bytes`). They are passed to the callbacks in the zero-copy `TsParser`. They provide efficient iterators over programs and streams (e.g., `PatRef::programs()`, `PmtRef::streams()`).

### `StreamType`

Enum representing various stream types defined in MPEG-2 and other standards.

#### Methods

- `is_video()` - Check if this is a video stream type
- `is_audio()` - Check if this is an audio stream type

## Error Handling

The crate provides detailed error types through `TsError`:

- `InvalidPacketSize` - TS packet is not 188 bytes
- `InvalidSyncByte` - Sync byte is not 0x47
- `InsufficientData` - Not enough data to parse
- `InvalidTableId` - Wrong table ID for PAT/PMT
- `ParseError` - General parsing errors

## Running the Example

```bash
cd crates/ts
cargo run --example zero_copy_demo
```

## Testing

Run the test suite:

```bash
cd crates/ts
cargo test
```

## License

This crate is licensed under MIT OR Apache-2.0. 