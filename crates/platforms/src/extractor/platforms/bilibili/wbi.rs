use md5::Digest;
use reqwest::{Client, header::USER_AGENT};
use serde::Deserialize;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, watch};
const CACHE_EXPIRATION: Duration = Duration::from_secs(2 * 60 * 60); // 2 hours

#[derive(Clone, Debug)]
pub struct WbiKeys {
    img_key: String,
    sub_key: String,
    timestamp: Instant,
}

impl WbiKeys {
    fn new(img_key: String, sub_key: String) -> Self {
        Self {
            img_key,
            sub_key,
            timestamp: Instant::now(),
        }
    }

    fn is_stale(&self) -> bool {
        self.timestamp.elapsed() > CACHE_EXPIRATION
    }
}

use std::sync::LazyLock;
static WBI_KEYS_WATCH: LazyLock<(watch::Sender<Option<WbiKeys>>, Mutex<()>)> =
    LazyLock::new(|| {
        let (tx, _) = watch::channel(None);
        (tx, Mutex::new(()))
    });

static WBI_KEYS_RX: LazyLock<watch::Receiver<Option<WbiKeys>>> =
    LazyLock::new(|| WBI_KEYS_WATCH.0.subscribe());

use crate::extractor::error::ExtractorError;
use crate::extractor::{default::DEFAULT_UA, platforms::bilibili::Bilibili};

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

#[derive(Deserialize)]
struct WbiImg {
    img_url: String,
    sub_url: String,
}

#[derive(Deserialize)]
struct Data {
    wbi_img: WbiImg,
}

#[derive(Deserialize)]
struct ResWbi {
    data: Data,
}

// 对 imgKey 和 subKey 进行字符顺序打乱编码
fn get_mixin_key(orig: &[u8]) -> String {
    MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .map(|&i| orig[i] as char)
        .collect::<String>()
}

fn get_url_encoded(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // Unreserved characters that do not need to be encoded.
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                encoded.push(c);
            }
            // Characters that are explicitly filtered out and not included in the output.
            '!' | '\'' | '(' | ')' | '*' => {}
            // All other characters are percent-encoded.
            _ => {
                let mut buf = [0; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    encoded.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    encoded
}

// 为请求参数进行 wbi 签名
pub(super) fn encode_wbi(
    params: Vec<(&str, String)>,
    keys: WbiKeys,
) -> Result<String, ExtractorError> {
    let cur_time = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(t) => t.as_secs(),
        Err(_) => {
            return Err(ExtractorError::Other(
                "SystemTime before UNIX EPOCH!".to_string(),
            ));
        }
    };
    Ok(_encode_wbi(
        params,
        (&keys.img_key, &keys.sub_key),
        cur_time,
    ))
}

fn _encode_wbi(
    mut params: Vec<(&str, String)>,
    (img_key, sub_key): (&str, &str),
    timestamp: u64,
) -> String {
    let mixin_key = get_mixin_key((img_key.to_owned() + sub_key).as_bytes());
    // 添加当前时间戳
    params.push(("wts", timestamp.to_string()));
    // 重新排序
    params.sort_by(|a, b| a.0.cmp(b.0));
    // 拼接参数
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", get_url_encoded(k), get_url_encoded(v)))
        .collect::<Vec<_>>()
        .join("&");
    // 计算签名
    let mut hasher = md5::Md5::new();
    hasher.update(query.clone() + &mixin_key);
    let md5_hash = hasher.finalize();
    let web_sign = format!("{md5_hash:x}");
    // 返回最终的 query
    query + &format!("&w_rid={web_sign}")
}

async fn fetch_new_keys(client: &Client) -> Result<WbiKeys, reqwest::Error> {
    let ResWbi {
        data: Data { wbi_img },
    } = client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .header(USER_AGENT, DEFAULT_UA)
        .header(reqwest::header::REFERER.to_string(), Bilibili::BASE_URL)
        .send()
        .await?
        .json::<ResWbi>()
        .await?;

    let img_key = take_filename(wbi_img.img_url).unwrap();
    let sub_key = take_filename(wbi_img.sub_url).unwrap();

    Ok(WbiKeys::new(img_key, sub_key))
}

pub(super) async fn get_wbi_keys(client: &Client) -> Result<WbiKeys, reqwest::Error> {
    let check_keys = || {
        let rx = WBI_KEYS_RX.clone();
        let keys = rx.borrow();
        if let Some(k) = &*keys {
            if !k.is_stale() {
                return Some(Ok(k.clone()));
            }
        }
        None
    };

    // Initial check
    if let Some(result) = check_keys() {
        return result;
    }

    // If cache is stale or empty, acquire lock to refresh.
    let _lock = WBI_KEYS_WATCH.1.lock().await;

    // Double-check after acquiring lock
    if let Some(result) = check_keys() {
        return result;
    }

    // If still stale, fetch and send the new keys.
    let new_keys = fetch_new_keys(client).await?;
    WBI_KEYS_WATCH.0.send(Some(new_keys.clone())).ok();
    Ok(new_keys)
}

fn take_filename(url: String) -> Option<String> {
    url.rsplit_once('/')
        .and_then(|(_, s)| s.rsplit_once('.'))
        .map(|(s, _)| s.to_string())
}

// #[tokio::main]
// async fn main() {
//     let keys = get_wbi_keys().await.unwrap();
//     let params = vec![
//         ("foo", String::from("114")),
//         ("bar", String::from("514")),
//         ("baz", String::from("1919810")),
//     ];
//     let query = encode_wbi(params, keys);
//     println!("{}", query);
// }

// 取自文档描述的测试用例
#[cfg(test)]
mod tests {
    use crate::extractor::default::default_client;

    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_get_wbi_keys() {
        let keys = get_wbi_keys(&default_client()).await;
        assert!(keys.is_ok());
        println!("{keys:?}");
    }

    #[test]
    fn test_get_filename() {
        assert_eq!(
            take_filename(
                "https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png".to_string()
            ),
            Some("7cd084941338484aae1ad9425b84077c".to_string())
        );
    }

    #[test]
    fn test_get_mixin_key() {
        let concat_key =
            "7cd084941338484aae1ad9425b84077c".to_string() + "4932caff0ff746eab6f01bf08b70ac45";
        assert_eq!(
            get_mixin_key(concat_key.as_bytes()),
            "ea1db124af3c7062474693fa704f4ff8"
        );
    }

    #[test]
    fn test_encode_wbi() {
        let params = vec![
            ("foo", String::from("114")),
            ("bar", String::from("514")),
            ("zab", String::from("1919810")),
        ];
        let keys = (
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
        );
        assert_eq!(
            _encode_wbi(params, keys, 1702204169),
            "bar=514&foo=114&wts=1702204169&zab=1919810&w_rid=8f6f2b5b3d485fe1886cec6a0be8c5d4"
                .to_string()
        )
    }
}
