//! Douyu STT (Serialized Text Transport) codec.
//!
//! This module implements the STT serialization format used by Douyu for danmu messages.
//! The format uses key-value pairs with special character escaping.
//!
//! ## Format Rules
//! - Key-value pairs are separated by `@=`
//! - Multiple key-value pairs are separated by `/`
//! - Character escaping:
//!   - `/` → `@S`
//!   - `@` → `@A`

use bytes::{BufMut, Bytes, BytesMut};
use rustc_hash::FxHashMap;

/// Magic number for client → server messages
const CLIENT_MAGIC: [u8; 4] = [0xb1, 0x02, 0x00, 0x00];

/// Magic number for server → client messages  
const SERVER_MAGIC: [u8; 4] = [0xb2, 0x02, 0x00, 0x00];

/// Heartbeat packet: type@=mrkl/
pub(crate) const HEARTBEAT: &[u8] = &[
    0x14, 0x00, 0x00, 0x00, // length = 20
    0x14, 0x00, 0x00, 0x00, // length = 20
    0xb1, 0x02, 0x00, 0x00, // type = 689
    0x74, 0x79, 0x70, 0x65, 0x40, 0x3d, 0x6d, 0x72, 0x6b, 0x6c, 0x2f, 0x00, // type@=mrkl/\0
];

/// Escape special characters in an STT value.
///
/// Escapes `@` as `@A` and `/` as `@S`.
pub fn stt_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '@' => result.push_str("@A"),
            '/' => result.push_str("@S"),
            _ => result.push(c),
        }
    }
    result
}

/// Unescape STT special characters.
///
/// Converts `@A` back to `@` and `@S` back to `/`.
pub fn stt_unescape(s: &str) -> String {
    s.replace("@S", "/").replace("@A", "@")
}

/// Encode a map of key-value pairs to STT format.
///
/// # Example
/// ```ignore
/// let mut map = HashMap::new();
/// map.insert("type", "loginreq");
/// map.insert("roomid", "123456");
/// let stt = stt_encode(&map);
/// // Result: "type@=loginreq/roomid@=123456/"
/// ```
pub fn stt_encode(map: &FxHashMap<&str, &str>) -> String {
    let mut result = String::new();
    for (key, value) in map {
        result.push_str(&stt_escape(key));
        result.push_str("@=");
        result.push_str(&stt_escape(value));
        result.push('/');
    }
    result
}

/// Decode an STT-formatted string to a map of key-value pairs.
///
/// # Example
/// ```ignore
/// let map = stt_decode("type@=loginreq/roomid@=123456/");
/// assert_eq!(map.get("type"), Some(&"loginreq".to_string()));
/// ```
pub fn stt_decode(data: &str) -> FxHashMap<String, String> {
    let mut map = FxHashMap::default();

    // Split by `/` and filter out empty parts
    for part in data.split('/') {
        if part.is_empty() {
            continue;
        }

        // Split by `@=` to get key-value pair
        if let Some((key, value)) = part.split_once("@=") {
            map.insert(stt_unescape(key), stt_unescape(value));
        }
    }

    map
}

/// Create a binary packet with proper headers for the Douyu protocol.
///
/// ## Packet Structure
/// ```text
/// | Length (4 bytes LE) | Length (4 bytes LE) | Magic (4 bytes) | STT Payload | Null byte |
/// ```
///
/// - The length field is `payload_length + 9` (4 bytes magic + 4 bytes length2 + 1 null)
/// - Magic number is 0xb1020000 for client messages
pub fn create_packet(message: &str) -> Bytes {
    let payload = message.as_bytes();
    // Length = magic(4) + payload_length + null(1) = payload_length + 5
    // But the format has length repeated, so: length = payload_length + 9
    let length = (payload.len() + 9) as u32;

    let mut buf = BytesMut::with_capacity(payload.len() + 13);

    // First length field (little-endian)
    buf.put_u32_le(length);
    // Second length field (little-endian) - same value
    buf.put_u32_le(length);
    // Magic number for client messages
    buf.put_slice(&CLIENT_MAGIC);
    // STT payload
    buf.put_slice(payload);
    // Null terminator
    buf.put_u8(0x00);

    buf.freeze()
}

/// Parse a binary packet and extract the STT payload.
///
/// Returns the payload string and the number of bytes consumed.
/// Returns None if the packet is incomplete or malformed.
pub fn parse_packet(data: &[u8]) -> Option<(String, usize)> {
    // Minimum packet size: 4 (len1) + 4 (len2) + 4 (magic) + 1 (null) = 13 bytes
    if data.len() < 13 {
        return None;
    }

    // Read length (little-endian)
    let length = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    // Total packet size is length + 4 (first length field)
    let total_size = length + 4;

    if data.len() < total_size {
        return None;
    }

    // Skip: len1(4) + len2(4) + magic(4) = 12 bytes
    // Payload ends at: total_size - 1 (null terminator)
    let payload_start = 12;
    let payload_end = total_size - 1;

    if payload_end <= payload_start {
        return Some((String::new(), total_size));
    }

    let payload = &data[payload_start..payload_end];

    // Convert to string (lossy for robustness)
    let payload_str = String::from_utf8_lossy(payload).to_string();

    Some((payload_str, total_size))
}

/// Parse multiple packets from a buffer.
///
/// Returns a vector of decoded payloads.
pub fn parse_packets(data: &[u8]) -> Vec<String> {
    let mut packets = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        match parse_packet(&data[offset..]) {
            Some((payload, consumed)) => {
                if !payload.is_empty() {
                    packets.push(payload);
                }
                offset += consumed;
            }
            None => break,
        }
    }

    packets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stt_escape() {
        assert_eq!(stt_escape("hello"), "hello");
        assert_eq!(stt_escape("hello@world"), "hello@Aworld");
        assert_eq!(stt_escape("hello/world"), "hello@Sworld");
        assert_eq!(stt_escape("@/"), "@A@S");
        assert_eq!(stt_escape(""), "");
    }

    #[test]
    fn test_stt_unescape() {
        assert_eq!(stt_unescape("hello"), "hello");
        assert_eq!(stt_unescape("hello@Aworld"), "hello@world");
        assert_eq!(stt_unescape("hello@Sworld"), "hello/world");
        assert_eq!(stt_unescape("@A@S"), "@/");
        assert_eq!(stt_unescape(""), "");
    }

    #[test]
    fn test_stt_encode() {
        let mut map = FxHashMap::default();
        map.insert("type", "loginreq");
        map.insert("roomid", "123456");

        let encoded = stt_encode(&map);

        // The order is not guaranteed, so check for presence
        assert!(encoded.contains("type@=loginreq/"));
        assert!(encoded.contains("roomid@=123456/"));
    }

    #[test]
    fn test_stt_decode() {
        let map = stt_decode("type@=loginreq/roomid@=123456/");

        assert_eq!(map.get("type"), Some(&"loginreq".to_string()));
        assert_eq!(map.get("roomid"), Some(&"123456".to_string()));
    }

    #[test]
    fn test_stt_decode_with_escaping() {
        let map = stt_decode("key@=value@Awith@Sslash/");

        assert_eq!(map.get("key"), Some(&"value@with/slash".to_string()));
    }

    #[test]
    fn test_stt_encode_decode_roundtrip() {
        let mut original = FxHashMap::default();
        original.insert("type", "test");
        original.insert("content", "hello");

        let encoded = stt_encode(&original);
        let decoded = stt_decode(&encoded);

        assert_eq!(decoded.get("type"), Some(&"test".to_string()));
        assert_eq!(decoded.get("content"), Some(&"hello".to_string()));
    }

    #[test]
    fn test_create_packet() {
        let message = "type@=loginreq/";
        let packet = create_packet(message);

        // Verify length
        let length = u32::from_le_bytes([packet[0], packet[1], packet[2], packet[3]]);
        assert_eq!(length as usize, message.len() + 9);

        // Verify magic number
        assert_eq!(&packet[8..12], &CLIENT_MAGIC);

        // Verify null terminator
        assert_eq!(packet[packet.len() - 1], 0x00);
    }

    #[test]
    fn test_parse_packet() {
        let message = "type@=test/";
        let packet = create_packet(message);

        let (payload, consumed) = parse_packet(&packet).unwrap();

        assert_eq!(payload, message);
        assert_eq!(consumed, packet.len());
    }

    #[test]
    fn test_parse_packet_roundtrip() {
        let message = "type@=chatmsg/nn@=TestUser/txt@=Hello World!/";
        let packet = create_packet(message);

        let (payload, _) = parse_packet(&packet).unwrap();
        let decoded = stt_decode(&payload);

        assert_eq!(decoded.get("type"), Some(&"chatmsg".to_string()));
        assert_eq!(decoded.get("nn"), Some(&"TestUser".to_string()));
        assert_eq!(decoded.get("txt"), Some(&"Hello World!".to_string()));
    }

    #[test]
    fn test_parse_packets_multiple() {
        let msg1 = "type@=first/";
        let msg2 = "type@=second/";

        let mut combined = create_packet(msg1).to_vec();
        combined.extend_from_slice(&create_packet(msg2));

        let packets = parse_packets(&combined);

        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], msg1);
        assert_eq!(packets[1], msg2);
    }

    #[test]
    fn test_parse_packet_incomplete() {
        // Less than minimum size
        assert!(parse_packet(&[0x00, 0x01, 0x02]).is_none());

        // Header indicates more data than available
        let packet = create_packet("test");
        assert!(parse_packet(&packet[..packet.len() - 5]).is_none());
    }

    #[test]
    fn test_heartbeat_packet() {
        // Verify the HEARTBEAT constant is a valid packet
        let (payload, consumed) = parse_packet(HEARTBEAT).unwrap();

        assert_eq!(payload, "type@=mrkl/");
        assert_eq!(consumed, HEARTBEAT.len());

        // Verify the decoded message type
        let decoded = stt_decode(&payload);
        assert_eq!(decoded.get("type"), Some(&"mrkl".to_string()));
    }

    #[test]
    fn test_create_heartbeat_matches_constant() {
        // Verify that create_packet produces the same result as the HEARTBEAT constant
        let generated = create_packet("type@=mrkl/");

        assert_eq!(generated.as_ref(), HEARTBEAT);
    }
}
