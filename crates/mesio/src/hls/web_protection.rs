//! `EXT-X-BIGO-WEB-PROTECTION` segment scrambling.
//!
//! Bigo media playlists carry `#EXT-X-BIGO-WEB-PROTECTION:VERSION=1,SEED=<u32>`.
//! While the tag is present, the first 16 bytes of each of the first two
//! 188-byte TS packets of every segment (its PAT and PMT) are XORed with a
//! xorshift keystream derived from the seed. The handling is keyed on the tag,
//! not on the platform, so any origin sending it is descrambled.

use bytes::{Bytes, BytesMut};
use m3u8_rs::MediaPlaylist;
use tracing::debug;

const TAG: &str = "X-BIGO-WEB-PROTECTION";
const TS_PACKET_SIZE: usize = 188;
const SYNC_BYTE: u8 = 0x47;
const SCRAMBLED_PACKETS: usize = 2;
const SCRAMBLED_BYTES: usize = 16;

/// The `SEED` of the playlist's web-protection tag, if it has one that parses.
///
/// m3u8-rs attaches an unknown tag to the segment that follows it rather than
/// to the playlist, so both places are searched.
pub(crate) fn playlist_seed(playlist: &MediaPlaylist) -> Option<u32> {
    playlist
        .unknown_tags
        .iter()
        .chain(playlist.segments.iter().flat_map(|s| s.unknown_tags.iter()))
        .find(|tag| tag.tag == TAG)
        .and_then(|tag| tag.rest.as_deref())
        .and_then(|rest| {
            rest.split(',')
                .find_map(|attr| attr.trim().strip_prefix("SEED="))
                .and_then(|seed| seed.trim().parse().ok())
        })
}

fn keystream(seed: u32) -> [[u8; SCRAMBLED_BYTES]; SCRAMBLED_PACKETS] {
    let mut masks = [[0u8; SCRAMBLED_BYTES]; SCRAMBLED_PACKETS];
    for (num, packet_mask) in (1u32..).zip(masks.iter_mut()) {
        let mut r = seed ^ num.wrapping_mul(2_654_435_769);
        if r == 0 {
            r = 1_831_565_813;
        }
        for mask in packet_mask.iter_mut() {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            *mask = match (r & 0xFF) as u8 {
                0 => 165,
                byte => byte,
            };
        }
    }
    masks
}

/// Descrambles the leading packets of a TS segment.
///
/// Applied only when both descrambled packets start with the TS sync byte, so
/// non-TS bodies, already-clear segments and a mismatched seed pass through
/// unchanged. A body shorter than two packets is left as is: it cannot be
/// verified, and a real segment is always far longer. Only the first
/// `2 * 188` bytes are rewritten; a uniquely owned buffer is reused in place.
pub(crate) fn descramble(data: Bytes, seed: u32) -> Bytes {
    let masks = keystream(seed);
    let scrambled_ts = data.len() >= SCRAMBLED_PACKETS * TS_PACKET_SIZE
        && masks
            .iter()
            .enumerate()
            .all(|(num, mask)| data[num * TS_PACKET_SIZE] ^ mask[0] == SYNC_BYTE);
    if !scrambled_ts {
        debug!(
            len = data.len(),
            "segment is not scrambled TS; leaving it unchanged"
        );
        return data;
    }
    let mut buf = data
        .try_into_mut()
        .unwrap_or_else(|shared| BytesMut::from(shared.as_ref()));
    for (num, mask) in masks.iter().enumerate() {
        let start = num * TS_PACKET_SIZE;
        for (byte, m) in buf[start..start + SCRAMBLED_BYTES].iter_mut().zip(mask) {
            *byte ^= m;
        }
    }
    buf.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn descrambles_bigo_pat_and_pmt() {
        let mut segment = vec![0u8; 3 * TS_PACKET_SIZE];
        segment[..12].copy_from_slice(&hex("63a0ef613b68d75bbaf5fbff"));
        segment[TS_PACKET_SIZE..TS_PACKET_SIZE + 12]
            .copy_from_slice(&hex("48d3cac24944c6eaaa712646"));
        segment[2 * TS_PACKET_SIZE] = SYNC_BYTE;

        let clear = descramble(Bytes::from(segment), 4_058_585_590);

        assert_eq!(clear[..12], hex("474000100000b00d0001c100")[..]);
        assert_eq!(
            clear[TS_PACKET_SIZE..TS_PACKET_SIZE + 12],
            hex("475001100002b0170001c100")[..]
        );
        assert_eq!(clear[2 * TS_PACKET_SIZE], SYNC_BYTE);
    }

    #[test]
    fn parses_seed_from_media_playlist() {
        let input = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
            #EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-BIGO-WEB-PROTECTION:VERSION=1,SEED=4058585590\n\
            #EXTINF:2.0,\n7.ts\n#EXTINF:2.0,\n8.ts\n";
        let m3u8_rs::Playlist::MediaPlaylist(playlist) =
            m3u8_rs::parse_playlist_res(input.as_bytes()).unwrap()
        else {
            panic!("expected media playlist");
        };
        assert_eq!(playlist_seed(&playlist), Some(4_058_585_590));
    }
}
