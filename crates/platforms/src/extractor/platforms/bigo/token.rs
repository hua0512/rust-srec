//! BigoCaptcha integrity token mint (`getFpToken` equivalent).
//!
//! 1. JSONP GET `/v1/webjs/t` → server time
//! 2. Fingerprint map (all strings, trunc 100) encrypted with OpenSSL-salted AES-256-CBC
//! 3. JSONP GET `/v1/webjs/status?data=` → server-minted token

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes::Aes256;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use cbc::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};
use parking_lot::Mutex as ParkingMutex;
use rand::{RngExt, rng};
use reqwest::Client;
use serde_json::{Map, Value};
use tracing::debug;

use crate::digest_to_hex;
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;

const PASSPHRASE: &str = "undefinedval0x01";
const SEC_HOST: &str = "https://sec.bigo.sg";
const DEFAULT_BUSINESS: &str = "bigolive-video";
const PROTOCOL_VER: &str = "2.0";
const VALUE_MAX: usize = 100;

type Aes256CbcEnc = cbc::Encryptor<Aes256>;

/// OpenSSL EVP_BytesToKey with MD5, 1 iteration (CryptoJS default).
pub fn evp_bytes_to_key(password: &[u8], salt: &[u8]) -> ([u8; 32], [u8; 16]) {
    let mut derived = Vec::with_capacity(48);
    let mut block = Vec::new();
    while derived.len() < 48 {
        let mut hasher = Md5::new();
        hasher.update(&block);
        hasher.update(password);
        hasher.update(salt);
        block = hasher.finalize().to_vec();
        derived.extend_from_slice(&block);
    }
    let mut key = [0u8; 32];
    let mut iv = [0u8; 16];
    key.copy_from_slice(&derived[..32]);
    iv.copy_from_slice(&derived[32..48]);
    (key, iv)
}

/// Base64(`Salted__` ‖ salt ‖ ciphertext) matching CryptoJS.AES.encrypt().toString().
pub fn aes_encrypt_openssl_salted(plaintext: &str, passphrase: &str) -> String {
    let mut salt = [0u8; 8];
    rng().fill(&mut salt);
    let (key, iv) = evp_bytes_to_key(passphrase.as_bytes(), &salt);
    let cipher = Aes256CbcEnc::new_from_slices(&key, &iv).expect("AES-256 key/iv length");
    let pt_len = plaintext.len();
    let padded_len = ((pt_len / 16) + 1) * 16;
    let mut buffer = vec![0u8; padded_len];
    buffer[..pt_len].copy_from_slice(plaintext.as_bytes());
    let encrypted = cipher
        .encrypt_padded::<Pkcs7>(&mut buffer, pt_len)
        .expect("AES encrypt");
    let mut out = Vec::with_capacity(16 + encrypted.len());
    out.extend_from_slice(b"Salted__");
    out.extend_from_slice(&salt);
    out.extend_from_slice(encrypted);
    B64.encode(out)
}

/// Extract JSON object from `callback({…});` or bare `{…}`.
pub fn parse_jsonp(body: &str) -> Result<Value, ExtractorError> {
    let body = body.trim();
    if let Some(start) = body.find('{') {
        // Match outermost object by scanning for the last closing brace after start.
        if let Some(end) = body.rfind('}')
            && end > start
        {
            let slice = &body[start..=end];
            return serde_json::from_str(slice).map_err(|e| {
                ExtractorError::ValidationError(format!("failed to parse JSONP JSON: {e}"))
            });
        }
    }
    Err(ExtractorError::ValidationError(format!(
        "unrecognized JSONP body: {}",
        body.chars().take(200).collect::<String>()
    )))
}

fn trunc(value: &str) -> String {
    value.chars().take(VALUE_MAX).collect()
}

fn jsonp_callback_name() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let r: u32 = rng().random_range(1000..100_000);
    format!("jsonpcallback_{ms}_{r}")
}

fn random_hex_md5() -> String {
    let mut entropy = [0u8; 16];
    rng().fill(&mut entropy);
    let mut hasher = Md5::new();
    hasher.update(entropy);
    digest_to_hex(&hasher.finalize())
}

/// Build a camouflaged desktop fingerprint map (all values strings, truncated).
pub fn build_fingerprint(at_time: &str, camouflage: bool) -> Map<String, Value> {
    let mut fp = Map::new();

    if camouflage {
        let mut rng = rng();
        let chrome_v = rng.random_range(120..=131);
        let platforms = [
            (
                "Win32",
                "Windows NT 10.0; Win64; x64",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{v}.0.0.0 Safari/537.36",
            ),
            (
                "MacIntel",
                "Macintosh; Intel Mac OS X 10_15_7",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{v}.0.0.0 Safari/537.36",
            ),
            (
                "Linux x86_64",
                "X11; Linux x86_64",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{v}.0.0.0 Safari/537.36",
            ),
        ];
        let (gu, _os, ua_tpl) = platforms[rng.random_range(0..platforms.len())];
        let ua = ua_tpl.replace("{v}", &chrome_v.to_string());
        let screens = [(1920, 1080, 1040), (2560, 1440, 1400), (1366, 768, 728)];
        let (w, h, avail_h) = screens[rng.random_range(0..screens.len())];
        let timezones = [
            (0, "UTC"),
            (-120, "Europe/Madrid"),
            (-480, "Asia/Shanghai"),
            (-540, "Asia/Tokyo"),
            (300, "America/New_York"),
        ];
        let (tz_off, tz_name) = timezones[rng.random_range(0..timezones.len())];
        let hw = [4, 8, 12, 16][rng.random_range(0..4)];
        let mem = [4, 8, 16][rng.random_range(0..3)];
        let lang = ["en-US", "en-GB", "ja", "ko", "zh-CN"][rng.random_range(0..5)];
        let fonts = "Arial,Helvetica,Times,Times New Roman";
        let plugins = "PDF Viewer,Portable Document Format,application/pdf,pdf";
        let webgl = "Google Inc. (NVIDIA)~ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11)";
        let audio = format!("{:.14}", rng.random_range(35.0..130.0));
        let canvas_seed = random_hex_md5();
        let canvas_b64 = format!(
            "iVBORw0KGgoAAAANSUhEUgAA{}",
            B64.encode(canvas_seed.as_bytes())
        );

        insert_str(&mut fp, "wc", "false");
        insert_str(&mut fp, "rk", "");
        insert_str(&mut fp, "wg", "false");
        insert_str(&mut fp, "wk", "false");
        insert_str(&mut fp, "wl", "false");
        insert_str(&mut fp, "wp", "true");
        insert_str(&mut fp, "wx", "false");
        insert_str(&mut fp, "dz", &trunc(&ua));
        insert_str(&mut fp, "pu", "false");
        insert_str(&mut fp, "gs", lang);
        insert_str(&mut fp, "kd", "24");
        insert_str(&mut fp, "vk", &mem.to_string());
        insert_str(&mut fp, "gz", &hw.to_string());
        insert_str(&mut fp, "ec", &format!("{w},{h}"));
        insert_str(&mut fp, "tr", &format!("{w},{avail_h}"));
        insert_str(&mut fp, "lb", &tz_off.to_string());
        insert_str(&mut fp, "mo", tz_name);
        insert_str(&mut fp, "io", "true");
        insert_str(&mut fp, "wz", "true");
        insert_str(&mut fp, "mx", "true");
        insert_str(&mut fp, "gb", "false");
        insert_str(&mut fp, "nx", "false");
        insert_str(&mut fp, "cp", "not available");
        insert_str(&mut fp, "gu", gu);
        insert_str(&mut fp, "ya", &trunc(plugins));
        insert_str(
            &mut fp,
            "mq",
            &trunc(&format!(
                "canvas winding:yes,canvas fp:data:image/png;base64,{canvas_b64}"
            )),
        );
        insert_str(
            &mut fp,
            "ix",
            &trunc(&format!("data:image/png;base64,{canvas_b64}")),
        );
        insert_str(&mut fp, "dd", &trunc(webgl));
        insert_str(&mut fp, "vd", "false");
        insert_str(&mut fp, "cm", "false");
        insert_str(&mut fp, "ey", "false");
        insert_str(&mut fp, "ui", "false");
        insert_str(&mut fp, "nb", "false");
        insert_str(&mut fp, "lu", "0,false,false");
        insert_str(&mut fp, "ww", fonts);
        insert_str(&mut fp, "ni", &audio);
        insert_str(&mut fp, "dr", &random_hex_md5());
    } else {
        insert_str(&mut fp, "wc", "false");
        insert_str(&mut fp, "rk", "");
        insert_str(&mut fp, "wg", "false");
        insert_str(&mut fp, "wk", "false");
        insert_str(&mut fp, "wl", "false");
        insert_str(&mut fp, "wp", "true");
        insert_str(&mut fp, "wx", "false");
        insert_str(&mut fp, "dz", &trunc(DEFAULT_UA));
        insert_str(&mut fp, "pu", "false");
        insert_str(&mut fp, "gs", "en-US");
        insert_str(&mut fp, "kd", "24");
        insert_str(&mut fp, "vk", "8");
        insert_str(&mut fp, "gz", "8");
        insert_str(&mut fp, "ec", "1920,1080");
        insert_str(&mut fp, "tr", "1920,1040");
        insert_str(&mut fp, "lb", "0");
        insert_str(&mut fp, "mo", "UTC");
        insert_str(&mut fp, "io", "true");
        insert_str(&mut fp, "wz", "true");
        insert_str(&mut fp, "mx", "true");
        insert_str(&mut fp, "gb", "false");
        insert_str(&mut fp, "nx", "false");
        insert_str(&mut fp, "cp", "not available");
        insert_str(&mut fp, "gu", "Linux x86_64");
        insert_str(
            &mut fp,
            "ya",
            &trunc("PDF Viewer,Portable Document Format,application/pdf,pdf"),
        );
        insert_str(
            &mut fp,
            "mq",
            &trunc("canvas winding:yes,canvas fp:data:image/png;base64,iVBORw0KGgo="),
        );
        insert_str(&mut fp, "ix", &trunc("data:image/png;base64,iVBORw0KGgo="));
        insert_str(
            &mut fp,
            "dd",
            &trunc("Google Inc. (Google)~ANGLE (Google, Vulkan 1.3.0)"),
        );
        insert_str(&mut fp, "vd", "false");
        insert_str(&mut fp, "cm", "false");
        insert_str(&mut fp, "ey", "false");
        insert_str(&mut fp, "ui", "false");
        insert_str(&mut fp, "nb", "false");
        insert_str(&mut fp, "lu", "0,false,false");
        insert_str(&mut fp, "ww", "Arial,Helvetica,Times,Times New Roman");
        insert_str(&mut fp, "ni", "124.04347527516074");
        insert_str(&mut fp, "dr", &random_hex_md5());
    }

    insert_str(&mut fp, "business", DEFAULT_BUSINESS);
    insert_str(&mut fp, "scene", "");
    insert_str(&mut fp, "at_time", at_time);
    insert_str(&mut fp, "ver", PROTOCOL_VER);
    fp
}

fn insert_str(map: &mut Map<String, Value>, key: &str, value: &str) {
    map.insert(key.to_string(), Value::String(value.to_string()));
}

async fn fetch_server_time(client: &Client, ua: &str) -> Result<String, ExtractorError> {
    let cb = jsonp_callback_name();
    let url = format!("{SEC_HOST}/v1/webjs/t?callback=&callback={cb}");
    let body = client
        .get(&url)
        .header("User-Agent", ua)
        .header("Referer", "https://www.bigo.tv/")
        .header("Accept", "*/*")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let data = parse_jsonp(&body)?;
    let code = data.get("code");
    let ok = code
        .map(|c| c.as_i64() == Some(0) || c.as_str() == Some("0") || c.is_null())
        .unwrap_or(true);
    if !ok {
        return Err(ExtractorError::ValidationError(format!(
            "bigo /webjs/t failed: {data}"
        )));
    }
    data.get("time")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        })
        .ok_or_else(|| {
            ExtractorError::ValidationError(format!("bigo /webjs/t missing time: {data}"))
        })
}

async fn submit_fingerprint(
    client: &Client,
    cipher_b64: &str,
    ua: &str,
    urlencode_data: bool,
) -> Result<Value, ExtractorError> {
    let cb = jsonp_callback_name();
    let data_param = if urlencode_data {
        percent_encoding::utf8_percent_encode(cipher_b64, percent_encoding::NON_ALPHANUMERIC)
            .to_string()
    } else {
        cipher_b64.to_string()
    };
    let url = format!("{SEC_HOST}/v1/webjs/status?data={data_param}&callback={cb}");
    let body = client
        .get(&url)
        .header("User-Agent", ua)
        .header("Referer", "https://www.bigo.tv/")
        .header("Accept", "*/*")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    parse_jsonp(&body)
}

/// Mint a fresh Bigo integrity token. Fingerprint is camouflaged by default.
pub async fn mint_token(client: &Client) -> Result<String, ExtractorError> {
    mint_token_with_options(client, true, DEFAULT_UA).await
}

pub async fn mint_token_with_options(
    client: &Client,
    camouflage: bool,
    ua: &str,
) -> Result<String, ExtractorError> {
    // When camouflage is on, build_fingerprint picks a random UA into `dz`;
    // HTTP headers still use the provided `ua` (caller typically DEFAULT_UA).
    let at_time = fetch_server_time(client, ua).await?;
    let fp = build_fingerprint(&at_time, camouflage);
    let plain = serde_json::to_string(&Value::Object(fp)).map_err(|e| {
        ExtractorError::ValidationError(format!("fingerprint serialize failed: {e}"))
    })?;
    let cipher = aes_encrypt_openssl_salted(&plain, PASSPHRASE);

    let mut result = submit_fingerprint(client, &cipher, ua, true).await?;
    let code_ok = |v: &Value| {
        v.get("code")
            .map(|c| c.as_i64() == Some(0) || c.as_str() == Some("0"))
            .unwrap_or(false)
    };
    if !code_ok(&result) {
        debug!(
            ?result,
            "bigo token mint: first status failed, retry without urlencode"
        );
        result = submit_fingerprint(client, &cipher, ua, false).await?;
    }

    let token = result
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if token.is_none() || !code_ok(&result) {
        return Err(ExtractorError::ValidationError(format!(
            "bigo token mint failed: {result}"
        )));
    }
    Ok(token.unwrap())
}

/// Process-local token cache (TTL + max uses) for multi-poll reuse.
pub struct TokenPool {
    inner: ParkingMutex<TokenPoolState>,
    ttl: Duration,
    max_uses: u32,
}

struct TokenPoolState {
    token: Option<String>,
    born: Option<Instant>,
    uses: u32,
}

impl Default for TokenPool {
    fn default() -> Self {
        Self::new(Duration::from_secs(120), 50)
    }
}

impl TokenPool {
    pub fn new(ttl: Duration, max_uses: u32) -> Self {
        Self {
            inner: ParkingMutex::new(TokenPoolState {
                token: None,
                born: None,
                uses: 0,
            }),
            ttl,
            max_uses,
        }
    }

    pub async fn get(&self, client: &Client) -> Result<String, ExtractorError> {
        {
            let mut state = self.inner.lock();
            if let Some(born) = state.born
                && born.elapsed() < self.ttl
                && state.uses < self.max_uses
                && let Some(token) = state.token.clone()
            {
                state.uses = state.uses.saturating_add(1);
                return Ok(token);
            }
        }

        let token = mint_token(client).await?;
        let mut state = self.inner.lock();
        state.token = Some(token.clone());
        state.born = Some(Instant::now());
        state.uses = 1;
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evp_bytes_to_key_stable() {
        let (key, iv) = evp_bytes_to_key(b"undefinedval0x01", b"12345678");
        assert_eq!(
            digest_to_hex(&key),
            "2e8659e5b6d14b258d16d7a5cee673c01ff7c35ada1cef113c96b315115b26a9"
        );
        assert_eq!(digest_to_hex(&iv), "b5bc371c83633a20302cb2972f961721");
        let (key2, iv2) = evp_bytes_to_key(b"undefinedval0x01", b"12345678");
        assert_eq!(key, key2);
        assert_eq!(iv, iv2);
    }

    #[test]
    fn aes_encrypt_starts_with_openssl_prefix() {
        let cipher = aes_encrypt_openssl_salted(r#"{"a":"1"}"#, PASSPHRASE);
        let raw = B64.decode(&cipher).expect("b64");
        assert!(raw.starts_with(b"Salted__"));
        assert!(cipher.starts_with("U2FsdGVkX1") || B64.encode(b"Salted__").starts_with("U2F"));
        // CryptoJS base64 of Salted__ is U2FsdGVkX1
        assert!(cipher.starts_with("U2FsdGVkX1"));
    }

    #[test]
    fn parse_jsonp_callback_and_bare() {
        let v = parse_jsonp(r#"jsonpcallback_1_2({"code":0,"time":"abc"});"#).unwrap();
        assert_eq!(v["code"], 0);
        assert_eq!(v["time"], "abc");

        let v2 = parse_jsonp(r#"{"code":0,"token":"xyz"}"#).unwrap();
        assert_eq!(v2["token"], "xyz");
    }

    #[test]
    fn fingerprint_truncation() {
        let fp = build_fingerprint("t1", false);
        for (k, v) in &fp {
            if let Value::String(s) = v {
                assert!(
                    s.chars().count() <= VALUE_MAX,
                    "key {k} exceeds VALUE_MAX: {}",
                    s.chars().count()
                );
            }
        }
        assert_eq!(
            fp.get("business").and_then(|v| v.as_str()),
            Some(DEFAULT_BUSINESS)
        );
        assert_eq!(fp.get("ver").and_then(|v| v.as_str()), Some(PROTOCOL_VER));
        assert_eq!(fp.get("at_time").and_then(|v| v.as_str()), Some("t1"));
    }

    #[test]
    fn fingerprint_camouflage_has_required_keys() {
        let fp = build_fingerprint("t2", true);
        for key in ["wc", "dz", "dr", "business", "at_time", "ver", "mq", "ni"] {
            assert!(fp.contains_key(key), "missing {key}");
        }
    }
}
