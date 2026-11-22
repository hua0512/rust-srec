//! Binary frame framing and bounded Zlib/Brotli decompression for Bilibili danmaku.

use byteorder::{BigEndian, ByteOrder};
use bytes::Bytes;
use flate2::read::ZlibDecoder;
use tracing::debug;

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{MAX_DECOMPRESSED_FRAME_BYTES, read_bounded};

/// Heartbeat packet (operation = 2)
/// Header: len=31, header_len=16, ver=1, op=2, seq=1
/// Body: "[object Object]"
pub(super) const HEARTBEAT: &[u8] = &[
    0x00, 0x00, 0x00, 0x1f, // packet length = 31
    0x00, 0x10, // header length = 16
    0x00, 0x01, // version = 1
    0x00, 0x00, 0x00, 0x02, // operation = 2 (heartbeat)
    0x00, 0x00, 0x00, 0x01, // sequence = 1
    // "[object Object]"
    0x5b, 0x6f, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x20, 0x4f, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x5d,
];

/// Operation codes
#[expect(
    dead_code,
    reason = "retained for protocol variants and forward-compatible response handling"
)]
pub(super) mod op {
    pub const HEARTBEAT_REPLY: u32 = 3;
    pub const NOTIFICATION: u32 = 5;
    pub const AUTH: u32 = 7;
    pub const AUTH_REPLY: u32 = 8;
}

/// Protocol versions
pub(super) mod ver {
    pub const RAW_JSON: u16 = 0;
    pub const POPULARITY: u16 = 1;
    pub const ZLIB: u16 = 2;
    pub const BROTLI: u16 = 3;
}

/// Decoded packet
#[derive(Debug)]
pub(super) struct DecodedPacket {
    pub operation: u32,
    /// Slice of the frame (or of the decompressed buffer) the packet came
    /// from; refcounted, so packets never copy their body.
    pub body: Bytes,
}

/// Build a packet with the given body and operation code.
pub(super) fn build_packet(body: &[u8], operation: u32) -> Vec<u8> {
    let packet_len = 16 + body.len();
    let mut packet = Vec::with_capacity(packet_len);

    // Header
    packet.extend_from_slice(&(packet_len as u32).to_be_bytes()); // packet length
    packet.extend_from_slice(&16u16.to_be_bytes()); // header length
    packet.extend_from_slice(&1u16.to_be_bytes()); // version
    packet.extend_from_slice(&operation.to_be_bytes()); // operation
    packet.extend_from_slice(&1u32.to_be_bytes()); // sequence

    // Body
    packet.extend_from_slice(body);

    packet
}

/// Decode packets from `data`, decompressing top-level Zlib/Brotli envelopes
/// when `allow_compressed` is true and rejecting nested compressed frames.
/// An envelope that fails to decompress is skipped.
pub(super) fn decode_packets(data: &Bytes, allow_compressed: bool) -> Result<Vec<DecodedPacket>> {
    let mut packets = Vec::new();
    let mut offset = 0;

    while offset + 16 <= data.len() {
        let packet_len = BigEndian::read_u32(&data[offset..offset + 4]) as usize;
        let header_len = BigEndian::read_u16(&data[offset + 4..offset + 6]) as usize;
        let version = BigEndian::read_u16(&data[offset + 6..offset + 8]);
        let operation = BigEndian::read_u32(&data[offset + 8..offset + 12]);

        // Every frame has at least a 16-byte header, and `header_len` must fit
        // within `packet_len` without overflowing `offset + packet_len`.
        if packet_len < 16
            || header_len < 16
            || header_len > packet_len
            || packet_len > data.len() - offset
        {
            break;
        }

        let body_start = offset + header_len;
        let body_end = offset + packet_len;
        let body = &data[body_start..body_end];

        match version {
            ver::ZLIB => {
                if !allow_compressed {
                    return Err(DanmakuError::protocol(
                        "Nested compressed Bilibili danmaku packet is forbidden",
                    ));
                }
                if let Some(decompressed) = inflate_or_skip(decompress_zlib(body))? {
                    packets.extend(decode_packets(&Bytes::from(decompressed), false)?);
                }
            }
            ver::BROTLI => {
                if !allow_compressed {
                    return Err(DanmakuError::protocol(
                        "Nested compressed Bilibili danmaku packet is forbidden",
                    ));
                }
                if let Some(decompressed) = inflate_or_skip(decompress_brotli(body))? {
                    packets.extend(decode_packets(&Bytes::from(decompressed), false)?);
                }
            }
            ver::RAW_JSON | ver::POPULARITY => {
                packets.push(DecodedPacket {
                    operation,
                    body: data.slice(body_start..body_end),
                });
            }
            _ => {
                debug!("Unknown protocol version: {}", version);
            }
        }

        offset += packet_len;
    }

    Ok(packets)
}

/// Separates a corrupt compressed body from one that breaks a limit. A body
/// that fails to inflate yields `None`, so the frame's other packets are still
/// delivered; one that exceeds the size limit stays an error.
fn inflate_or_skip(decompressed: Result<Vec<u8>>) -> Result<Option<Vec<u8>>> {
    match decompressed {
        Ok(decompressed) => Ok(Some(decompressed)),
        Err(DanmakuError::Io(error)) => {
            debug!(%error, "Skipping Bilibili danmaku packet that failed to decompress");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    read_bounded(
        ZlibDecoder::new(data),
        MAX_DECOMPRESSED_FRAME_BYTES,
        "Bilibili zlib",
    )
}

fn decompress_brotli(data: &[u8]) -> Result<Vec<u8>> {
    read_bounded(
        brotli::Decompressor::new(data, 4096),
        MAX_DECOMPRESSED_FRAME_BYTES,
        "Bilibili brotli",
    )
}
