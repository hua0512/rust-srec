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
ts = { path = "path/to/ts" }
```

### Basic Example

```rust
use ts::{TsParser, StreamType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = TsParser::new();
    
    // Read TS data from file or network
    let ts_data = std::fs::read("example.ts")?;
    
    // Parse the TS packets
    parser.parse_packets(&ts_data)?;
    
    // Access PAT information
    if let Some(pat) = parser.pat() {
        println!("Transport Stream ID: {}", pat.transport_stream_id);
        for program in &pat.programs {
            if program.program_number != 0 {
                println!("Program {}: PMT PID 0x{:04X}", 
                        program.program_number, program.pmt_pid);
            }
        }
    }
    
    // Access PMT information
    for (program_num, pmt) in parser.pmts() {
        println!("Program {} streams:", program_num);
        for stream in &pmt.streams {
            match stream.stream_type {
                StreamType::H264 => println!("  H.264 Video: PID 0x{:04X}", stream.elementary_pid),
                StreamType::AdtsAac => println!("  AAC Audio: PID 0x{:04X}", stream.elementary_pid),
                _ => println!("  Other: PID 0x{:04X}", stream.elementary_pid),
            }
        }
    }
    
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

### `TsParser`

The main parser struct for processing TS packets.

#### Methods

- `new()` - Create a new parser instance
- `parse_packets(&mut self, data: &[u8])` - Parse TS packets from byte data
- `pat(&self)` - Get the parsed PAT (if available)
- `pmts(&self)` - Get all parsed PMTs
- `pmt(&self, program_number: u16)` - Get a specific PMT by program number
- `reset(&mut self)` - Reset parser state

### `Pat` (Program Association Table)

Contains program information from the PAT.

#### Fields

- `transport_stream_id: u16` - Transport stream identifier
- `version_number: u8` - Version number
- `programs: Vec<PatProgram>` - List of programs

#### Methods

- `parse(data: &[u8])` - Parse PAT from PSI section data
- `network_pid()` - Get the Network PID
- `program_numbers()` - Get all program numbers
- `get_pmt_pid(program_number: u16)` - Get PMT PID for a program

### `Pmt` (Program Map Table)

Contains stream information for a program.

#### Fields

- `program_number: u16` - Program number
- `pcr_pid: u16` - PCR PID
- `streams: Vec<PmtStream>` - Elementary streams

#### Methods

- `parse(data: &[u8])` - Parse PMT from PSI section data
- `video_streams()` - Get all video streams
- `audio_streams()` - Get all audio streams
- `get_stream(pid: u16)` - Get stream by PID
- `get_all_pids()` - Get all PIDs used by this program

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
cargo run --example parse_ts
```

## Testing

Run the test suite:

```bash
cd crates/ts
cargo test
```

## License

This crate is licensed under MIT OR Apache-2.0. 