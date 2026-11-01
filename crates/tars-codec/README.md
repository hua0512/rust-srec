# TARS Codec

An encoder and decoder for the TARS (Tencent Application Request System) wire
protocol, built on the `bytes` crate. In this workspace it backs the Huya
extractor and danmaku client in `platforms-parser`.

## Features

- **Message framing**: `TarsCodec` implements `tokio_util::codec::{Decoder, Encoder}`
  for length-prefixed `TarsMessage` frames (request header plus a body of named
  raw payloads).
- **Value model**: `TarsValue` covers the TARS types (integers, floats, strings,
  structs, maps, lists, `SimpleList` byte arrays) and keeps binary payloads as
  `Bytes`.
- **Zero-copy strings**: `TarsValue::StringRef(Bytes)` holds string data without
  copying; `decode_response_zero_copy` decodes a frame in that mode.
- **Compact collections**: lists use `SmallVec`, structs and maps use `FxHashMap`.

## Usage

### Decoding a response frame

`TarsMessage::body` maps each field name to its raw TARS-encoded payload.
Decode a payload with `decode_tars_value`, or with `decode_tars_struct` when it
is a struct without the outer struct begin/end markers.

```rust
use bytes::BytesMut;
use tars_codec::{TarsError, decode_response, decode_tars_value};

fn read_frame(buffer: &mut BytesMut) -> Result<(), TarsError> {
    // `None` means the buffer does not yet hold a complete frame.
    let Some(message) = decode_response(buffer)? else {
        return Ok(());
    };

    for (name, payload) in &message.body {
        let value = decode_tars_value(payload.clone())?;
        if let Some(text) = value.as_str() {
            println!("{name}: {text}");
        }
    }
    Ok(())
}
```

`decode_response_from_bytes` and `decode_response_zero_copy` decode one complete
frame from owned `Bytes`; the zero-copy variant keeps strings as
`TarsValue::StringRef` instead of allocating `String`s.

### Encoding

- `encode_request(&TarsMessage)` encodes a framed request.
- `encode_tars_value(&TarsValue)` encodes a value; a `TarsValue::Struct` is
  written as its fields only.
- `encode_tars_value_wrapped(&TarsValue)` encodes a value with struct begin/end
  markers.
- `next_request_id()` returns a process-wide incrementing request ID.

### Value accessors

- `TarsValue::as_str()` borrows `String` and `StringRef` values as `&str`.
- `TarsValue::into_string()` returns an owned `String`, copying only `StringRef`
  data.
- `TarsValue::as_bytes()` borrows `SimpleList`, `Binary` and `StringRef` data.
- `TarsValue::as_i32()` reads `Byte`, `Short` and `Int` values.

## Example

`examples/allocation_demo.rs` compares the memory layout of `SmallVec` and
`Vec` list storage for `TarsValue`:

```bash
cargo run --locked -p tars-codec --example allocation_demo
```
