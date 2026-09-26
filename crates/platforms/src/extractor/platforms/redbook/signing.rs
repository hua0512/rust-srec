//! X-S request signing for the RedBook live-room H5 API.

use base64::{Engine, engine::general_purpose::STANDARD};
use md5::{Digest, Md5};

const ENVELOPE_ALPHABET: &[u8; 64] =
    b"ZmserbBoHQtNP+wOcza/LpngG8yJq42KWYj0DSfdikx3VT16IlUAFM97hECvuRX5";
const PAYLOAD_ALPHABET: &[u8; 64] =
    b"MfgqrsbcyzPQRStuvC7mn501HIJBo2DEFTKdeNOwxWXYZap89+/A4UVLhijkl63G";
const PAYLOAD_MASK: &[u8] = &[
    113, 163, 2, 37, 119, 147, 39, 29, 221, 39, 59, 206, 227, 228, 185, 141, 157, 121, 53, 225,
    218, 51, 245, 118, 94, 46, 168, 175, 182, 220, 119, 165, 26, 73, 157, 35, 182, 124, 32, 102, 0,
    37, 134, 12, 191, 19, 212, 84, 13, 146, 73, 127, 88, 104, 108, 87, 78, 80, 143, 70, 225, 149,
    99, 68, 243, 145, 57, 191, 79, 175, 34, 163, 238, 241, 32, 183, 146, 88, 20, 91, 47, 235, 81,
    147, 182, 71, 134, 105, 150, 18, 152, 231, 155, 237, 202, 100, 110, 26, 105, 58, 146, 97, 84,
    165, 167, 161, 189, 28, 240, 222, 219, 116, 47, 145, 122, 116, 122, 30, 56, 139, 35, 79, 34,
    119,
];

fn encode(bytes: &[u8], alphabet: &[u8; 64]) -> String {
    STANDARD
        .encode(bytes)
        .bytes()
        .map(|byte| {
            let index = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return '=',
            };
            char::from(alphabet[usize::from(index)])
        })
        .collect()
}

pub(super) fn sign(content: &str, a1: &str) -> String {
    sign_with_parameters(
        content,
        a1,
        chrono::Utc::now().timestamp_millis() as u64,
        rand::random(),
        rand::random_range(10..=50),
        rand::random_range(15..=50),
        rand::random_range(900..=1200),
    )
}

fn sign_with_parameters(
    content: &str,
    a1: &str,
    timestamp: u64,
    seed: u32,
    time_offset: u64,
    sequence: u32,
    window_properties: u32,
) -> String {
    // The wire payload has fixed field widths, little-endian integers and a
    // 52-byte cookie slot. Only the first eight MD5 bytes are included.
    let mut payload = [0u8; 124];
    payload[..4].copy_from_slice(&[119, 104, 96, 41]);
    payload[4..8].copy_from_slice(&seed.to_le_bytes());
    let mut fingerprint = timestamp.to_le_bytes();
    fingerprint[0] = fingerprint[1..]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    payload[8..16].copy_from_slice(&fingerprint.map(|byte| byte ^ 41));
    payload[16..24].copy_from_slice(&timestamp.saturating_sub(time_offset).to_le_bytes());
    payload[24..28].copy_from_slice(&sequence.to_le_bytes());
    payload[28..32].copy_from_slice(&window_properties.to_le_bytes());
    // Room-info paths and queries are ASCII after URL encoding.
    payload[32..36].copy_from_slice(&(content.len() as u32).to_le_bytes());
    let digest = Md5::digest(content.as_bytes());
    for (target, byte) in payload[36..44].iter_mut().zip(digest.iter()) {
        *target = byte ^ seed as u8;
    }
    payload[44] = 52;
    let cookie_len = a1.len().min(52);
    payload[45..45 + cookie_len].copy_from_slice(&a1.as_bytes()[..cookie_len]);
    payload[97] = 10;
    payload[98..108].copy_from_slice(b"xhs-pc-web");
    payload[108..111].copy_from_slice(&[1, 1, seed as u8 ^ 115]);
    // The protocol truncates the final checksum byte at the 124-byte boundary.
    payload[111..].copy_from_slice(&[249, 65, 103, 103, 201, 181, 131, 99, 94, 7, 68, 250, 132]);
    for (byte, mask) in payload.iter_mut().zip(PAYLOAD_MASK) {
        *byte ^= mask;
    }
    let encoded = encode(&payload, PAYLOAD_ALPHABET);
    let envelope = format!(
        r#"{{"x0":"4.2.6","x1":"xhs-pc-web","x2":"Windows","x3":"mns0301_{encoded}","x4":""}}"#
    );
    format!("XYS_{}", encode(envelope.as_bytes(), ENVELOPE_ALPHABET))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn decode(encoded: &str, alphabet: &[u8; 64]) -> Vec<u8> {
        let standard = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let encoded: String = encoded
            .bytes()
            .map(|byte| {
                if byte == b'=' {
                    '='
                } else {
                    char::from(standard[alphabet.iter().position(|b| *b == byte).unwrap()])
                }
            })
            .collect();
        STANDARD.decode(encoded).unwrap()
    }

    pub(crate) fn decode_request_fields(signature: &str) -> (u32, Vec<u8>, String) {
        let envelope: serde_json::Value = serde_json::from_slice(&decode(
            signature.strip_prefix("XYS_").unwrap(),
            ENVELOPE_ALPHABET,
        ))
        .unwrap();
        let mut payload = decode(
            envelope["x3"]
                .as_str()
                .unwrap()
                .strip_prefix("mns0301_")
                .unwrap(),
            PAYLOAD_ALPHABET,
        );
        for (byte, mask) in payload.iter_mut().zip(PAYLOAD_MASK) {
            *byte ^= mask;
        }
        let length = u32::from_le_bytes(payload[32..36].try_into().unwrap());
        let digest = payload[36..44]
            .iter()
            .map(|byte| byte ^ payload[4])
            .collect();
        let a1 = String::from_utf8(
            payload[45..97]
                .iter()
                .copied()
                .take_while(|b| *b != 0)
                .collect(),
        )
        .unwrap();
        (length, digest, a1)
    }

    #[test]
    fn matches_fixed_wire_signature_vectors() {
        let content = "/api/sns/red/live/h5/v1/room/current_room_info?room_id=room123&source=share_out_of_app";
        for (a1, expected) in [
            (
                "1221".to_string(),
                "XYS_2UQhPsHCH0c1Pjh9HjIj2erjwjQhyoPTqBPT49pjHjIj2eHjwjQgynEDJ74AHjIj2ePjwjQTJdPIPAZlg98yGLTlGFzT2fSpJFDhJ7Y9JUR9p9+VzezdyL+awepnzaRx2bSxcLWUy0pa+FDF8BH7JLEE4fMpz98cLgSIaLRM2dkCzppL2rQAPdz12/8jyrDM8r+M4r+Fc08MnaRc8BVhqD8YygkHq7+ywBkpGfMY8LzjaDSH+FRFzrQ3JM4ScSz+c9EIqMQCLDkcpnbLP9IUP7YDPBTnGFQP4gqMwepC//YHJeDROaHVHdWFH0ijHdF=",
            ),
            (
                "a".repeat(60),
                "XYS_2UQhPsHCH0c1Pjh9HjIj2erjwjQhyoPTqBPT49pjHjIj2eHjwjQgynEDJ74AHjIj2ePjwjQTJdPIPAZlg98yGLTlGFzT2fSpJFDhJ7Y9JUR9p9+VzezdyL+awepnzaRx2bSxcLWUy0pa+FDF8BH7JLEE4fMpz98cLnR8JSps4pQhG9zS2dbTaeYazBY7zdp987kQL/DIL9YIcnpM89zk8f8pwbQkPfMg4rTDqaRpNFQ787LUc/+HcfkDLbkAqaT+c9EIqMQCLDkcpnbLP9IUP7YDPBTnGFQP4gqMwepC//YHJeDROaHVHdWFH0ijHdF=",
            ),
        ] {
            assert_eq!(
                sign_with_parameters(content, &a1, 1_700_000_000_000, 0x8000_0000, 30, 33, 1050),
                expected
            );
        }
    }
}
