//! Android 8.2.2.0 playback signing.
//! The tables and block transform reproduce libmakeurl4.0.1.so.
//! Protocol reference: https://github.com/biliup/biliup/pull/1748.

mod data;
mod transform;

use std::collections::BTreeMap;

use md5::{Digest, Md5};

use data::{MAPS, SALTS};
use transform::transform_block;

pub(super) const DEFAULT_DID: &str = "10000000000000000000000000001511";
pub(super) type Params = BTreeMap<String, String>;

/// Merge sorted overrides while hashing raw canonical bytes. This avoids
/// cloning the parameter map and allocating a String for every key/value.
fn hash_canonical(hash: &mut Md5, params: &Params, excluded: &[&str], overrides: &[(&str, &str)]) {
    let mut first = true;
    let mut write = |key: &str, value: &str| {
        if !first {
            hash.update(b"&");
        }
        first = false;
        hash.update(key.as_bytes());
        hash.update(b"=");
        hash.update(value.as_bytes());
    };
    let mut replacements = overrides.iter().peekable();
    for (key, value) in params {
        while let Some(&&(replacement, value)) = replacements.peek()
            && replacement < key.as_str()
        {
            write(replacement, value);
            replacements.next();
        }
        if let Some(&&(replacement, value)) = replacements.peek()
            && replacement == key.as_str()
        {
            write(replacement, value);
            replacements.next();
        } else if !excluded.contains(&key.as_str()) {
            write(key, value);
        }
    }
    for &(key, value) in replacements {
        write(key, value);
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

fn csign(rid: u64, did: &str, timestamp: u64, params: &Params) -> [u8; 16] {
    let index = (rid % 200) as usize;
    let prefix = format!("{rid}{did}{timestamp}0103{}", SALTS[index]);
    let mut hash = Md5::new();
    hash.update(prefix.as_bytes());
    hash_canonical(
        &mut hash,
        params,
        &["csign", "amd", "host", "name_time"],
        &[("client_sys", "android"), ("cptl", "0103")],
    );
    let digest = hash.finalize();
    let mut key = [0; 10];
    for (dest, src) in key.iter_mut().zip(prefix.bytes()) {
        *dest = src;
    }
    let mut result = [0; 16];
    for (dest, block) in result
        .as_chunks_mut::<8>()
        .0
        .iter_mut()
        .zip(digest.as_chunks::<8>().0)
    {
        let transformed = transform_block(block, &key);
        for (dest, byte) in dest.iter_mut().zip(transformed) {
            *dest = MAPS[index][usize::from(byte)];
        }
    }
    result
}

fn amd(signature: &[u8; 16], did: &str) -> String {
    let words: [u32; 4] = std::array::from_fn(|i| {
        let start = i * 4;
        u32::from_le_bytes(std::array::from_fn(|j| signature[start + j]))
    });
    let mut state = [0; 88];
    state[8] = 1;
    state[12] = 1;
    let did = &did.as_bytes()[..did.len().min(36)];
    state[52..52 + did.len()].copy_from_slice(did);
    for block in state.as_chunks_mut::<8>().0 {
        let mut a = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut b = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let mut sum = 0_u32;
        for _ in 0..32 {
            sum = sum.wrapping_add(0x9e3779b9);
            a = a.wrapping_add(
                (b << 4).wrapping_add(words[0])
                    ^ b.wrapping_add(sum)
                    ^ (b >> 5).wrapping_add(words[1]),
            );
            b = b.wrapping_add(
                (a << 4).wrapping_add(words[2])
                    ^ a.wrapping_add(sum)
                    ^ (a >> 5).wrapping_add(words[3]),
            );
        }
        block[..4].copy_from_slice(&a.to_le_bytes());
        block[4..].copy_from_slice(&b.to_le_bytes());
    }
    hex(&state)
}

fn header_auth(rid: u64, timestamp: u64, params: &Params) -> String {
    let excluded = [
        "client_sys",
        "host",
        "name_time",
        "retryTimes",
        "auth_position",
        "replaceParameter",
        "get_params_order",
    ];
    // The playback endpoint accepts the relative path, without a leading slash
    // or hostname. The hostname-prefixed interceptor variant fails authentication.
    let mut hash = Md5::new();
    hash.update(format!("lapi/live/appGetPlayer/stream/{rid}?").as_bytes());
    let timestamp = timestamp.to_string();
    hash_canonical(
        &mut hash,
        params,
        &excluded,
        &[
            ("aid", "android1"),
            ("client_sys", "android"),
            ("time", &timestamp),
        ],
    );
    hash.update(b"vq47Hd9JUgfDCytC");
    hex(&hash.finalize())
}

/// Sign raw, sorted values before HTTP query encoding. All three signatures
/// must share the same device ID, timestamp, and playback parameters.
pub(super) fn sign(rid: u64, did: &str, timestamp: u64, params: &mut Params) -> String {
    let signature = csign(rid, did, timestamp, params);
    params.insert("csign".into(), hex(&signature));
    params.insert("amd".into(), amd(&signature, did));
    params.insert("cptl".into(), "0103".into());
    params.insert("client_sys".into(), "android".into());
    header_auth(rid, timestamp, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        [
            ("txdw", "0"),
            ("cdn", "hw"),
            ("token", ""),
            ("rate", "0"),
            ("hevc", "0"),
            ("ilow", "0"),
            ("iar", "0"),
            ("net", "WIFI"),
            ("device", "PHK110"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }

    #[test]
    fn signing_vectors() {
        #[derive(serde::Deserialize)]
        struct Vector {
            rid: u64,
            did: String,
            timestamp: u64,
            params: Params,
            csign: String,
            amd: String,
            auth: String,
        }
        // Golden vectors cover the native signing sample, raw UTF-8 values,
        // reserved parameter overrides, and device IDs truncated by amd.
        let vectors: Vec<Vector> =
            serde_json::from_str(include_str!("app_sign/reference_vectors.json")).unwrap();
        for (index, mut vector) in vectors.into_iter().enumerate() {
            let auth = sign(
                vector.rid,
                &vector.did,
                vector.timestamp,
                &mut vector.params,
            );
            assert_eq!(vector.params["csign"], vector.csign, "csign vector {index}");
            assert_eq!(vector.params["amd"], vector.amd, "amd vector {index}");
            assert_eq!(auth, vector.auth, "auth vector {index}");
        }
    }

    #[test]
    fn all_room_tables_match_golden_digest() {
        let mut digest = Md5::new();
        for i in 0..200_u64 {
            let did = if i % 2 == 0 {
                DEFAULT_DID
            } else {
                "ABCDEF12-3456-7890-ABCD-EF1234567890"
            };
            let mut params = params();
            params.insert("cdn".into(), ["hw", "tct", "hs"][(i % 3) as usize].into());
            params.insert("rate".into(), (i % 4).to_string());
            params.insert("hevc".into(), (i % 2).to_string());
            let auth = sign(1126800 + i, did, 1790312470 + i, &mut params);
            for value in [&params["csign"], &params["amd"], &auth] {
                digest.update(value.as_bytes());
            }
        }
        // Golden digest of csign || amd || auth across all 200 room tables
        // and both DID formats.
        assert_eq!(hex(&digest.finalize()), "8d4426b78030f7c5ad7c4665a27c9eb3");
    }
}
