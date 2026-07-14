//! Bigo Live guest WebSocket danmu provider.
//!
//! Protocol (receive-only guest):
//! 1. POST getWebSocketLink → uidToken / userId / deviceId
//! 2. WSS connect; server challenge (eid 256) → MD5-signed 79108 + LOGIN 512279
//! 3. LOGIN res 200 → enter room (1304) with studio roomId
//! 4. NORMAL_TEXT 2584: tag 1/2 chat, tag 6 gift
//! 5. Ping 791 every ~10s

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use md5::{Digest, Md5};
use parking_lot::RwLock;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::http::HeaderMap;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::ws_headers_origin_ua;
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::danmaku::{DanmuItem, DanmuMessage};
use crate::digest_to_hex;
use crate::extractor::default::{DEFAULT_UA, default_client};
use crate::extractor::platforms::bigo::URL_REGEX;
use crate::extractor::platforms::bigo::models::{WsLinkData, WsLinkResponse};
use crate::extractor::utils::capture_group_1_owned;

const WS_LINK_URL: &str = "https://ta.bigo.tv/official_website/studio/getWebSocketLink";
const WSS_URL: &str = "wss://wss.bigolive.tv/live/official/web";
const HEARTBEAT_INTERVAL_SECS: u64 = 10;

const EID_CHALLENGE: u32 = 256;
const EID_CHALLENGE_RESP: u32 = 79108;
const EID_LOGIN_REQ: u32 = 512279;
const EID_LOGIN_RES: u32 = 512535;
const EID_ENTER_REQ: u32 = 1304;
const EID_ENTER_RES: u32 = 1560;
const EID_NORMAL_TEXT: u32 = 2584;
const EID_PING: u32 = 791;

const TAG_NORMAL: i64 = 1;
const TAG_DANMAKU: i64 = 2;
const TAG_GIFT: i64 = 6;

#[derive(Clone)]
struct GuestSession {
    device_id: String,
    user_id: String,
    uid_token: String,
}

/// Bigo danmu protocol state (guest session + password).
#[derive(Clone)]
pub struct BigoDanmuProtocol {
    client: Client,
    session: Arc<RwLock<Option<GuestSession>>>,
    room_password: Arc<RwLock<Option<String>>>,
    login_sent: Arc<AtomicBool>,
    enter_sent: Arc<AtomicBool>,
}

impl Default for BigoDanmuProtocol {
    fn default() -> Self {
        Self {
            client: default_client(),
            session: Arc::new(RwLock::new(None)),
            room_password: Arc::new(RwLock::new(None)),
            login_sent: Arc::new(AtomicBool::new(false)),
            enter_sent: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl BigoDanmuProtocol {
    pub fn new() -> Self {
        Self::default()
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn now_sec() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn pack(eid: u32, body: &Value) -> Message {
        let compact = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
        Message::Text(format!("{eid}{compact}").into())
    }

    fn parse_frame(text: &str) -> Option<(u32, Value)> {
        let i = text.find('{')?;
        let eid: u32 = text[..i].trim().parse().ok()?;
        let body: Value = serde_json::from_str(&text[i..]).ok()?;
        Some((eid, body))
    }

    fn decode_payload_content(content_b64: &str) -> Option<Value> {
        let raw = B64.decode(content_b64).ok()?;
        if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
            return Some(v);
        }
        if let Ok(s) = std::str::from_utf8(&raw)
            && let Ok(v) = serde_json::from_str::<Value>(s)
        {
            return Some(v);
        }
        // latin-1 fallback
        let s: String = raw.iter().map(|&b| b as char).collect();
        serde_json::from_str(&s).ok()
    }

    fn challenge_response(challenge: &str) -> Message {
        let ts = Self::now_sec().to_string();
        let tail = if challenge.len() >= 8 {
            &challenge[challenge.len() - 8..]
        } else {
            challenge
        };
        let material = format!("60#4#5#{ts}#1#1#1#1#{tail}");
        let mut hasher = Md5::new();
        hasher.update(material.as_bytes());
        let sign = digest_to_hex(&hasher.finalize());
        let body = json!({
            "appId": "60",
            "osType": "4",
            "clientVersion": "5",
            "timeStamp": ts,
            "nonce": "1",
            "reservedForSecurity": "1",
            "appSign": "1",
            "redundancy": "1",
            "sign": sign,
        });
        Self::pack(EID_CHALLENGE_RESP, &body)
    }

    fn login_message(session: &GuestSession) -> Message {
        let body = json!({
            "uid": session.user_id,
            "cookie": session.uid_token,
            "secret": "0",
            "userName": "0",
            "deviceId": session.device_id,
            "userFlag": "0",
            "status": "0",
            "password": "0",
            "sdkVersion": "0",
            "displayType": "0",
            "pbVersion": "0",
            "lang": "cn",
            "loginLevel": "0",
            "clientVersionCode": "0",
            "clientType": "7",
            "clientOsVer": "0",
            "netConf": {
                "clientIp": "0",
                "proxySwitch": "0",
                "proxyTimestamp": "0",
                "mcc": "0",
                "mnc": "0",
                "countryCode": "CN"
            }
        });
        Self::pack(EID_LOGIN_REQ, &body)
    }

    fn enter_message(session: &GuestSession, room_id: &str, secret_key: &str) -> Message {
        let body = json!({
            "secretKey": secret_key,
            "seqId": Self::now_ms().to_string(),
            "roomId": room_id,
            "reserver": "1",
            "clientVersion": "0",
            "clientType": "7",
            "version": "15",
            "deviceid": session.device_id,
            "other": []
        });
        Self::pack(EID_ENTER_REQ, &body)
    }

    fn ping_message() -> Message {
        let body = json!({
            "status": "0",
            "seqid": Self::now_ms().to_string(),
            "flag": "0",
            "roomId": "0",
            "ownerStatus": "0",
            "micUid": "0"
        });
        Self::pack(EID_PING, &body)
    }

    fn normal_text_to_items(frame: &Value) -> Vec<DanmuItem> {
        let payload = match frame.get("payload") {
            Some(p) => p,
            None => return vec![],
        };
        let tag = payload
            .get("tag")
            .and_then(|t| {
                t.as_i64()
                    .or_else(|| t.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        let uid = payload
            .get("uid")
            .or_else(|| frame.get("from_uid"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "0".to_string());
        let seq = payload
            .get("seqId")
            .or_else(|| frame.get("seqId"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| format!("bigo-{}", Self::now_ms()));

        let decoded = payload
            .get("content")
            .and_then(|c| c.as_str())
            .and_then(Self::decode_payload_content);

        match tag {
            TAG_NORMAL | TAG_DANMAKU => {
                let Some(obj) = decoded.as_ref().and_then(|v| v.as_object()) else {
                    return vec![];
                };
                let nick = obj
                    .get("n")
                    .or_else(|| obj.get("nick"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(uid.as_str())
                    .to_string();
                let msg = obj
                    .get("m")
                    .or_else(|| obj.get("msg"))
                    .or_else(|| obj.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if msg.is_empty() {
                    return vec![];
                }
                let mut danmu = DanmuMessage::chat(seq, uid, nick, msg);
                if let Some(grade) = payload.get("grade") {
                    danmu = danmu.with_metadata("grade", grade.clone());
                }
                vec![DanmuItem::Message(danmu)]
            }
            TAG_GIFT => {
                let Some(obj) = decoded.as_ref().and_then(|v| v.as_object()) else {
                    return vec![];
                };
                let nick = obj
                    .get("n")
                    .or_else(|| obj.get("uname"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(uid.as_str())
                    .to_string();
                let gift_name = obj
                    .get("giftName")
                    .or_else(|| obj.get("gift_name"))
                    .or_else(|| obj.get("m"))
                    .or_else(|| obj.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("gift")
                    .to_string();
                let count = obj
                    .get("c")
                    .or_else(|| obj.get("num"))
                    .or_else(|| obj.get("count"))
                    .and_then(|v| {
                        v.as_u64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    })
                    .unwrap_or(1) as u32;
                let danmu = DanmuMessage::gift(seq, uid, nick, gift_name, count.max(1));
                vec![DanmuItem::Message(danmu)]
            }
            _ => vec![],
        }
    }

    async fn fetch_guest(&self) -> Result<GuestSession> {
        let mut entropy = [0u8; 16];
        rand::rng().fill(&mut entropy);
        let mut hasher = Md5::new();
        hasher.update(entropy);
        let device_id = format!("web_{}", digest_to_hex(&hasher.finalize()));

        let response = self
            .client
            .post(WS_LINK_URL)
            .header("User-Agent", DEFAULT_UA)
            .header("Referer", "https://www.bigo.tv/")
            .header("Origin", "https://www.bigo.tv")
            .header("Accept", "application/json")
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=UTF-8",
            )
            .form(&[("deviceId", device_id.as_str())])
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("getWebSocketLink failed: {e}")))?;

        if !response.status().is_success() {
            return Err(DanmakuError::connection(format!(
                "getWebSocketLink HTTP {}",
                response.status()
            )));
        }

        let payload: WsLinkResponse = response
            .json()
            .await
            .map_err(|e| DanmakuError::protocol(format!("getWebSocketLink parse: {e}")))?;

        if !payload.is_success() {
            return Err(DanmakuError::protocol(format!(
                "getWebSocketLink error: code={:?}",
                payload.code
            )));
        }

        let data: WsLinkData = payload.data.unwrap_or_default();
        let uid_token = data
            .uid_token
            .filter(|s| !s.is_empty())
            .map(|s| s.replace("###VER2", ""))
            .ok_or_else(|| DanmakuError::protocol("getWebSocketLink missing uidToken"))?;
        let user_id = data
            .user_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| DanmakuError::protocol("getWebSocketLink missing userId"))?;
        let device_id = data
            .device_id
            .filter(|s| !s.is_empty())
            .unwrap_or(device_id);

        Ok(GuestSession {
            device_id,
            user_id,
            uid_token,
        })
    }
}

// rand::RngExt is used in fetch_guest
use rand::RngExt;

impl DanmuProtocol for BigoDanmuProtocol {
    fn platform(&self) -> &str {
        "bigo"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        // siteId from URL — studio roomId must come from MediaInfo.extras.
        capture_group_1_owned(&URL_REGEX, url)
    }

    fn configure_connection(
        &mut self,
        _cookies: Option<&str>,
        extras: Option<&HashMap<String, String>>,
    ) {
        if let Some(e) = extras {
            let pwd = e
                .get("stream_password")
                .or_else(|| e.get("password"))
                .cloned()
                .filter(|s| !s.is_empty());
            *self.room_password.write() = pwd;
        }
        self.login_sent.store(false, Ordering::SeqCst);
        self.enter_sent.store(false, Ordering::SeqCst);
    }

    async fn websocket_url(&self, _room_id: &str) -> Result<String> {
        let guest = self.fetch_guest().await?;
        debug!(
            user_id = %guest.user_id,
            device_id = %guest.device_id,
            "bigo guest session ready"
        );
        *self.session.write() = Some(guest);
        self.login_sent.store(false, Ordering::SeqCst);
        self.enter_sent.store(false, Ordering::SeqCst);
        Ok(WSS_URL.to_string())
    }

    fn headers(&self, _room_id: &str) -> HeaderMap {
        ws_headers_origin_ua("https://www.bigo.tv", DEFAULT_UA)
    }

    fn send_cookie_header(&self) -> bool {
        false
    }

    async fn handshake_messages(&self, _room_id: &str) -> Result<Vec<Message>> {
        // Challenge-driven; optional delayed login if server never challenges.
        // We intentionally return empty here — login is sent on challenge or
        // would need a timer outside the trait. Python waits 2s without challenge;
        // research shows challenge usually arrives first.
        Ok(vec![])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        Some(Self::ping_message())
    }

    fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(HEARTBEAT_INTERVAL_SECS)
    }

    async fn decode_message(
        &self,
        message: &Message,
        room_id: &str,
        tx: &mpsc::Sender<Message>,
    ) -> Result<Vec<DanmuItem>> {
        let text = match message {
            Message::Text(t) => t.as_str(),
            Message::Binary(b) => match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => return Ok(vec![]),
            },
            Message::Ping(_) | Message::Pong(_) => return Ok(vec![]),
            Message::Close(frame) => {
                debug!(?frame, "bigo ws close");
                return Err(DanmakuError::connection("Connection closed by server"));
            }
            _ => return Ok(vec![]),
        };

        let Some((eid, data)) = Self::parse_frame(text) else {
            debug!(%text, "bigo unparseable frame");
            return Ok(vec![]);
        };

        match eid {
            EID_CHALLENGE => {
                if let Some(challenge) = data.get("challenge").and_then(|c| c.as_str()) {
                    let _ = tx.send(Self::challenge_response(challenge)).await;
                    if !self.login_sent.swap(true, Ordering::SeqCst) {
                        let session = self.session.read().clone();
                        if let Some(session) = session {
                            let _ = tx.send(Self::login_message(&session)).await;
                        }
                    }
                }
                Ok(vec![])
            }
            EID_LOGIN_RES => {
                let res = data
                    .get("res")
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                if res == "200" {
                    debug!("bigo login ok");
                    if !self.enter_sent.swap(true, Ordering::SeqCst) {
                        let session = self.session.read().clone();
                        let secret = self
                            .room_password
                            .read()
                            .clone()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "0".to_string());
                        if let Some(session) = session {
                            let _ = tx
                                .send(Self::enter_message(&session, room_id, &secret))
                                .await;
                        }
                    }
                } else {
                    warn!(%res, "bigo login failed");
                }
                Ok(vec![])
            }
            EID_ENTER_RES => {
                let code = data
                    .get("resCode")
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                if code == "200" {
                    debug!(%room_id, "bigo enter room ok");
                } else {
                    warn!(%code, %room_id, "bigo enter room failed");
                }
                Ok(vec![])
            }
            EID_NORMAL_TEXT => Ok(Self::normal_text_to_items(&data)),
            EID_PING => Ok(vec![]),
            _ => {
                debug!(eid, "bigo ignored frame");
                Ok(vec![])
            }
        }
    }
}

/// Bigo danmu provider type alias.
pub type BigoDanmuProvider = WebSocketDanmuProvider<BigoDanmuProtocol>;

/// Creates a new Bigo danmu provider.
pub fn create_bigo_danmu_provider() -> BigoDanmuProvider {
    WebSocketDanmuProvider::with_protocol(BigoDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_matching() {
        let p = BigoDanmuProtocol::default();
        assert!(p.supports_url("https://www.bigo.tv/80104"));
        assert!(p.supports_url("https://www.bigo.tv/ja/80104"));
        assert!(!p.supports_url("https://www.twitch.tv/foo"));
    }

    #[test]
    fn parse_frame_and_pack() {
        let msg = BigoDanmuProtocol::pack(256, &json!({"challenge":"abc"}));
        if let Message::Text(t) = msg {
            let (eid, body) = BigoDanmuProtocol::parse_frame(t.as_str()).unwrap();
            assert_eq!(eid, 256);
            assert_eq!(body["challenge"], "abc");
        } else {
            panic!("expected text");
        }
    }

    #[test]
    fn challenge_sign_material() {
        // Just ensure packing challenge response produces text with sign field.
        let msg = BigoDanmuProtocol::challenge_response("12345678abcdefgh");
        if let Message::Text(t) = msg {
            assert!(t.starts_with("79108{"));
            assert!(t.contains("\"sign\":"));
        } else {
            panic!("expected text");
        }
    }

    #[test]
    fn decode_chat_and_gift() {
        let chat_content = B64.encode(br#"{"n":"Alice","m":"hello","a":"0","b":"0"}"#);
        let frame = json!({
            "from_uid": "1",
            "room_id": "r1",
            "payload": {
                "seqId": "s1",
                "uid": "1",
                "grade": "5",
                "tag": "1",
                "content": chat_content
            }
        });
        let items = BigoDanmuProtocol::normal_text_to_items(&frame);
        assert_eq!(items.len(), 1);
        if let DanmuItem::Message(m) = &items[0] {
            assert_eq!(m.username, "Alice");
            assert_eq!(m.content, "hello");
        } else {
            panic!("expected message");
        }

        let gift_content = B64.encode(br#"{"n":"Bob","m":"Rose","c":"3"}"#);
        let frame = json!({
            "payload": {
                "seqId": "s2",
                "uid": "2",
                "tag": 6,
                "content": gift_content
            }
        });
        let items = BigoDanmuProtocol::normal_text_to_items(&frame);
        assert_eq!(items.len(), 1);
        if let DanmuItem::Message(m) = &items[0] {
            assert_eq!(m.message_type, crate::danmaku::message::DanmuType::Gift);
            assert!(m.content.contains("Rose"));
        } else {
            panic!("expected gift");
        }
    }

    /// Live integration: guest login → enter studio roomId → receive frames.
    /// Run with:
    ///   cargo test -p platforms-parser bigo::danmu::tests::test_live_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_live_connection() {
        use crate::danmaku::provider::{ConnectionConfig, DanmuProvider};
        use crate::extractor::default::default_client;
        use crate::extractor::platform_extractor::PlatformExtractor;
        use crate::extractor::platforms::bigo::Bigo;

        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        // Resolve studio roomId from a live room (not siteId).
        let extractor = Bigo::new(
            "https://www.bigo.tv/221338632".to_string(),
            default_client(),
            None,
            None,
        );
        let media = extractor.extract().await.expect("extract for room_id");
        assert!(media.is_live, "need a live room for danmu enter");
        let room_id = media
            .extras
            .as_ref()
            .and_then(|e| e.get("room_id"))
            .cloned()
            .expect("room_id extras");
        println!("connecting danmu room_id={room_id}");

        let provider = create_bigo_danmu_provider();
        let mut connection = provider
            .connect(&room_id, ConnectionConfig::default())
            .await
            .expect("danmu connect failed");

        let mut message_count = 0u32;
        let mut control_count = 0u32;
        let timeout = tokio::time::Duration::from_secs(25);

        let result = tokio::time::timeout(timeout, async {
            loop {
                match provider.receive(&connection).await {
                    Ok(Some(item)) => match item {
                        crate::danmaku::DanmuItem::Message(danmu) => {
                            println!(
                                "[{}] {}: {}",
                                danmu.timestamp.format("%H:%M:%S"),
                                danmu.username,
                                danmu.content
                            );
                            message_count += 1;
                        }
                        crate::danmaku::DanmuItem::Control(control) => {
                            println!("[control] {control:?}");
                            control_count += 1;
                        }
                    },
                    Ok(None) => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                    Err(e) => {
                        println!("receive error: {e}");
                        break;
                    }
                }
                // Quiet rooms may have zero chat; connection staying open is success.
                if message_count >= 3 {
                    break;
                }
            }
        })
        .await;

        let _ = provider.disconnect(&mut connection).await;

        match result {
            Ok(_) => println!("danmu loop finished"),
            Err(_) => println!("danmu wait timed out after {timeout:?} (may be a quiet room)"),
        }
        println!("messages={message_count} controls={control_count}");
        // Connect succeeded if we did not panic above. Chat volume is not guaranteed
        // (quiet rooms can stay open with zero messages).
    }
}
