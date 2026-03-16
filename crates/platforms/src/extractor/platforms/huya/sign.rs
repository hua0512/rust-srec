use base64::prelude::*;
use md5::{Digest, Md5};
use rand::seq::IndexedRandom;
use rand::{Rng, RngExt};
use std::time::{SystemTime, UNIX_EPOCH};
use url::form_urlencoded;

use crate::extractor::error::ExtractorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum HuyaPlatform {
    HuyaPcExe = 0,
    HuyaAdr = 2,
    HuyaIos = 3,
    TvHuyaNftv = 10,
    HuyaWebH5 = 100,
    MiniApp = 102,
    Wap = 103,
    HuyaLiveShareH5 = 104,
}

impl HuyaPlatform {
    pub(crate) fn all_variants() -> &'static [HuyaPlatform] {
        &[
            HuyaPlatform::HuyaPcExe,
            HuyaPlatform::HuyaAdr,
            HuyaPlatform::HuyaIos,
            HuyaPlatform::TvHuyaNftv,
            HuyaPlatform::HuyaWebH5,
            HuyaPlatform::MiniApp,
            HuyaPlatform::Wap,
            HuyaPlatform::HuyaLiveShareH5,
        ]
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            HuyaPlatform::HuyaPcExe => "huya_pc_exe",
            HuyaPlatform::HuyaAdr => "huya_adr",
            HuyaPlatform::HuyaIos => "huya_ios",
            HuyaPlatform::TvHuyaNftv => "tv_huya_nftv",
            HuyaPlatform::HuyaWebH5 => "huya_webh5",
            HuyaPlatform::MiniApp => "tars_mp",
            HuyaPlatform::Wap => "tars_mobile",
            HuyaPlatform::HuyaLiveShareH5 => "huya_liveshareh5",
        }
    }

    pub(crate) fn short_name(&self) -> &'static str {
        let name = self.name();
        name.split_once('_').map_or(name, |(_, after)| after)
    }

    pub(crate) fn as_pair(&self) -> (&'static str, u32) {
        (self.name(), *self as u32)
    }

    pub(crate) fn get_random() -> Self {
        let mut rng = rand::rng();
        *Self::all_variants().choose(&mut rng).unwrap()
    }

    pub(crate) fn generate_ua(&self) -> String {
        let mut rng = rand::rng();

        let mut version = match self {
            HuyaPlatform::HuyaAdr
            | HuyaPlatform::HuyaIos
            | HuyaPlatform::MiniApp
            | HuyaPlatform::Wap => String::from("13.1.0"),
            HuyaPlatform::TvHuyaNftv => String::from("2.6.10"),
            HuyaPlatform::HuyaPcExe => String::from("7080002"),
            _ => String::from("0.0.0"),
        };

        let channel = match self {
            HuyaPlatform::HuyaWebH5 => "websocket",
            _ => "official",
        };

        if matches!(self, HuyaPlatform::HuyaAdr | HuyaPlatform::TvHuyaNftv) {
            let build = rng.random_range(3000..=5000);
            version.push_str(&format!(".{}", build));
        }

        let mut ua = format!("{}&{}&{}", self.short_name(), version, channel);

        if matches!(self, HuyaPlatform::HuyaAdr | HuyaPlatform::TvHuyaNftv) {
            let api_level = rng.random_range(28..=36);
            ua.push_str(&format!("&{}", api_level));
        }

        ua
    }
}

fn rotl64(t: u64) -> u64 {
    let lower = (t as u32).rotate_left(8);
    (t & !0xFFFF_FFFF) | (lower as u64)
}

/// Generate a random UID for Huya authentication
fn generate_random_uid(rng: &mut impl Rng) -> u64 {
    // "1234" + 4 random digits → 12340000..12349999
    1234_0000 + rng.random_range(0..10_000)
}

fn generate_random_uuid() -> u64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let mut rng = rand::rng();
    ((now_ms % 10_000_000_000) * 1_000 + rng.random_range(0..1_000)) % 4_294_967_295
}

pub fn get_anticode(
    stream_name: &str,
    anti_code: &str,
    uid: Option<u64>,
    random_platform: bool,
) -> Result<String, ExtractorError> {
    let mut fm_enc = None;
    let mut fs = None;
    for (k, v) in form_urlencoded::parse(anti_code.as_bytes()) {
        match k.as_ref() {
            "fm" => fm_enc = Some(v.into_owned()),
            "fs" => fs = Some(v.into_owned()),
            _ => {}
        }
    }

    // no fm → no computation needed
    let fm_enc = match fm_enc {
        Some(fm) => fm,
        None => return Ok(anti_code.to_string()),
    };

    // get platform and its id
    let platform = if random_platform {
        HuyaPlatform::get_random()
    } else {
        HuyaPlatform::HuyaPcExe
    };
    let (ctype, platform_id) = platform.as_pair();
    let is_wap = matches!(platform, HuyaPlatform::Wap);

    let calc_start_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();

    let mut rng = rand::rng();

    // Generate or use provided UID
    let uid = uid
        .filter(|&u| u != 0)
        .unwrap_or_else(|| generate_random_uid(&mut rng));

    let seq_id = uid + (calc_start_time * 1000.0) as u64;

    // md5(seqid | ctype | platformType)
    let mut hasher = Md5::new();
    hasher.update(format!("{}|{}|{}", seq_id, ctype, platform_id));
    let secret_hash = format!("{:x}", hasher.finalize());

    // User param differs by platform:
    //   web → convertUid (rotl64-rotated)
    //   wap → raw uid
    let calc_uid = if is_wap { uid } else { rotl64(uid) };

    // Decode FM to extract secret prefix
    let fm_decoded_bytes = BASE64_STANDARD
        .decode(&fm_enc)
        .map_err(|e| ExtractorError::ValidationError(format!("base64 decode error: {e}")))?;
    let fm_decoded_str = String::from_utf8_lossy(&fm_decoded_bytes);
    let secret_prefix = fm_decoded_str.split('_').next().unwrap_or("");

    // ws_time: 1 day expiration
    let ws_time_val = (calc_start_time as u64) + 86_400;
    let ws_time = format!("{:x}", ws_time_val);

    // Fill FM template:
    //   _fm = "SecretPrefix_$0_$1_$2_$3"
    //   $0 → userParam (convertUid for web, raw uid for wap)
    //   $1 → streamName
    //   $2 → hash (md5 of seqid|ctype|platformType)
    //   $3 → wsTime

    // wsSecret = md5("prefix_userParam_streamName_hash_wsTime")
    let mut hasher2 = Md5::new();
    hasher2.update(format!(
        "{}_{}_{}_{}_{}",
        secret_prefix, calc_uid, stream_name, secret_hash, ws_time
    ));
    let ws_secret = format!("{:x}", hasher2.finalize());

    // Construct query string
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("wsSecret", &ws_secret);
    serializer.append_pair("wsTime", &ws_time);
    serializer.append_pair("seqid", &seq_id.to_string());
    serializer.append_pair("ctype", ctype);
    serializer.append_pair("ver", "1");
    if let Some(fs) = &fs {
        serializer.append_pair("fs", fs);
    }
    serializer.append_pair("fm", &fm_enc);
    serializer.append_pair("t", &platform_id.to_string());

    if is_wap {
        serializer.append_pair("uid", &uid.to_string());
        serializer.append_pair("uuid", &generate_random_uuid().to_string());
    } else {
        serializer.append_pair("u", &calc_uid.to_string());
    }

    Ok(serializer.finish())
}

#[cfg(test)]
mod tests {
    use crate::extractor::platforms::huya::get_anticode;

    #[test]
    fn test_build_query() {
        let stream_name = "test_stream";
        // fm needs to decode to something with '_'
        // "abc_def" -> base64 -> YWJjX2RlZg==
        let fm_val = "YWJjX2RlZg==";
        // fm is usually url encoded in anti_code
        // YWJjX2RlZg%3D%3D
        let anti_code = format!(
            "wsSecret=old&wsTime=old&seqid=old&ctype=old&ver=1&fs=fsval&fm={}&t=100",
            "YWJjX2RlZg%3D%3D"
        );

        // build_query
        let result = get_anticode(stream_name, &anti_code, Some(12345), false);
        assert!(result.is_ok());
        let new_query = result.unwrap();
        println!("New query: {}", new_query);

        let mut parsed: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (k, v) in url::form_urlencoded::parse(new_query.as_bytes()).into_owned() {
            parsed.entry(k).or_default().push(v);
        }
        assert!(parsed.contains_key("wsSecret"));
        assert!(parsed.contains_key("wsTime"));
        assert!(parsed.contains_key("seqid"));
        assert!(parsed.contains_key("ctype"));
        assert!(parsed.contains_key("fm"));
        // Check if fm value matches input (decoded)
        assert_eq!(parsed.get("fm").unwrap()[0], fm_val);
    }
}
