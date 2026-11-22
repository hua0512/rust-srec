//! Bilibili (哔哩哔哩) danmu provider.
//!
//! Implements danmu collection for Bilibili live streaming using the generic
//! WebSocket provider with binary protocol and Brotli/Zlib compression.

mod command;
mod packet;

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_tungstenite::tungstenite::http::HeaderMap;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::DanmuItem;
use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::ws_headers_origin_referer_ua;
use crate::danmaku::websocket::{
    DanmuProtocol, DanmuProtocolFactory, DanmuProtocolOutput, WebSocketDanmuProvider,
};
use crate::extractor::default::{DEFAULT_UA, default_client};

use super::URL_REGEX;
use super::cookie_utils::{extract_cookie_value, strip_refresh_token};
use super::utils::generate_fake_buvid3;
use super::wbi::{encode_wbi, get_wbi_keys};
use crate::extractor::utils::capture_group_1_owned;
use packet::{DecodedPacket, HEARTBEAT, build_packet, decode_packets, op};

/// Default WebSocket URL
const DEFAULT_WS_URL: &str = "wss://broadcastlv.chat.bilibili.com/sub";

/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Room init API response
#[derive(Debug, Deserialize)]
struct RoomInitResponse {
    code: i32,
    data: Option<RoomInitData>,
}

#[derive(Debug, Deserialize)]
struct RoomInitData {
    room_id: u64,
}

/// Authentication data sent to WebSocket
#[derive(Debug, Serialize)]
struct AuthData {
    uid: u64,
    roomid: u64,
    protover: u8,
    platform: &'static str,
    #[serde(rename = "type")]
    auth_type: u8,
    key: String,
}

/// Bilibili Danmu Protocol Implementation
#[derive(Clone)]
pub struct BilibiliDanmuProtocol {
    client: Client,
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
    uid: Option<u64>,
    connection_cookies: Option<String>,
    /// Cached `(requested_room_id, real_room_id, token)` resolved in `websocket_url`
    /// and consumed by `handshake_messages` to avoid duplicate HTTP + WBI calls.
    resolved_session: Option<(String, u64, String)>,
}

impl Default for BilibiliDanmuProtocol {
    fn default() -> Self {
        Self {
            client: default_client(),
            cookies: None,
            uid: None,
            connection_cookies: None,
            resolved_session: None,
        }
    }
}

impl BilibiliDanmuProtocol {
    /// Create a new BilibiliDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new BilibiliDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            client: default_client(),
            cookies: Some(cookies.into()),
            uid: None,
            connection_cookies: None,
            resolved_session: None,
        }
    }

    fn build_cookie_header(user_cookies: Option<&str>, fallback_buvid3: &str) -> String {
        let mut buvid3_value: Option<String> = None;
        let mut other_parts: Vec<String> = Vec::new();

        if let Some(user_cookies) = user_cookies {
            for part in user_cookies.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }

                let mut kv = part.splitn(2, '=');
                let name = kv.next().unwrap_or("").trim();
                let value = kv.next().map(str::trim);

                if name.eq_ignore_ascii_case("buvid3") {
                    if buvid3_value.is_none() && matches!(value, Some(v) if !v.is_empty()) {
                        buvid3_value = value.map(ToString::to_string);
                    }
                    continue;
                }

                other_parts.push(part.to_string());
            }
        }

        let buvid3_value = buvid3_value.unwrap_or_else(|| fallback_buvid3.to_string());

        if other_parts.is_empty() {
            format!("buvid3={}", buvid3_value)
        } else {
            format!("buvid3={}; {}", buvid3_value, other_parts.join("; "))
        }
    }

    fn http_cookie_header(&self) -> Option<String> {
        if let Some(cookies) = self.connection_cookies.as_deref() {
            return Some(strip_refresh_token(cookies));
        }
        self.cookies
            .as_deref()
            .map(|c| strip_refresh_token(&self.normalize_cookies(c)))
    }

    /// Get real room ID from short ID.
    async fn get_real_room_id(&self, short_id: &str) -> Result<u64> {
        let url = format!(
            "https://api.live.bilibili.com/room/v1/Room/room_init?id={}",
            short_id
        );

        let resp: RoomInitResponse = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::REFERER, "https://live.bilibili.com")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to get room info: {}", e)))?
            .json()
            .await
            .map_err(|e| DanmakuError::protocol(format!("Failed to parse room info: {}", e)))?;

        if resp.code != 0 {
            return Err(DanmakuError::protocol(
                "Room init API returned error".to_string(),
            ));
        }

        resp.data
            .map(|d| d.room_id)
            .ok_or_else(|| DanmakuError::protocol("No room data in response".to_string()))
    }

    /// Get danmaku connection info (WebSocket URL and token).
    async fn get_danmu_info(&self, room_id: u64) -> Result<(String, String)> {
        // Build params
        let params = vec![
            ("id", room_id.to_string()),
            ("type", "0".to_string()),
            ("web_location", "444.8".to_string()),
        ];

        // Sign with WBI
        let keys = get_wbi_keys(&self.client)
            .await
            .map_err(|e| DanmakuError::protocol(format!("Failed to get WBI keys: {}", e)))?;

        let query_string = encode_wbi(params, keys)
            .map_err(|e| DanmakuError::protocol(format!("Failed to encode WBI: {}", e)))?;

        let url = format!(
            "https://api.live.bilibili.com/xlive/web-room/v1/index/getDanmuInfo?{}",
            query_string
        );

        debug!("getDanmuInfo URL: {}", url);

        // Make request
        let mut req = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::REFERER, "https://live.bilibili.com");
        if let Some(cookie_header) = self.http_cookie_header() {
            req = req.header(reqwest::header::COOKIE, cookie_header);
        }

        let response = req
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to get danmu info: {}", e)))?
            .text()
            .await
            .map_err(|e| DanmakuError::protocol(format!("Failed to read response: {}", e)))?;

        // Parse JSON response
        let json: Value = serde_json::from_str(&response)
            .map_err(|e| DanmakuError::protocol(format!("Invalid JSON: {}", e)))?;

        // Check if API returned an error
        let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            debug!(
                "getDanmuInfo returned error code {}: {}, using default WebSocket URL",
                code, msg
            );
            return Ok((DEFAULT_WS_URL.to_string(), String::new()));
        }

        // Parse successful response
        let data = json
            .get("data")
            .ok_or_else(|| DanmakuError::protocol("Missing data field in response".to_string()))?;

        let token = data
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let ws_url = data
            .get("host_list")
            .and_then(|v| v.as_array())
            .and_then(|list| list.first())
            .and_then(|host| {
                let h = host.get("host")?.as_str()?;
                let p = host.get("wss_port")?.as_u64()?;
                Some(format!("wss://{}:{}/sub", h, p))
            })
            .unwrap_or_else(|| DEFAULT_WS_URL.to_string());

        debug!("Got token and WebSocket URL: {}", ws_url);
        Ok((ws_url, token))
    }

    /// Build authentication packet.
    fn build_auth_packet(&self, room_id: u64, token: &str) -> Bytes {
        // If `uid` is set to a non-zero user mid, the token must be obtained with
        // matching authenticated cookies (otherwise the server may reset the connection).
        let uid = if self.uid.is_some()
            && self
                .connection_cookies
                .as_deref()
                .and_then(|c| extract_cookie_value(c, "SESSDATA"))
                .is_some()
        {
            self.uid.unwrap_or(0)
        } else {
            0
        };
        let auth_data = AuthData {
            uid,
            roomid: room_id,
            protover: 3, // Request Brotli compression
            platform: "web",
            auth_type: 2,
            key: token.to_string(),
        };

        // `AuthData` is fully serializable; in the unlikely event of serialization failure,
        // send an empty payload rather than panic.
        let json_data = serde_json::to_vec(&auth_data).unwrap_or_default();
        Bytes::from(build_packet(&json_data, op::AUTH))
    }

    /// Decode packets, handling compression.
    fn decode_packets(data: &Bytes, allow_compressed: bool) -> Result<Vec<DecodedPacket>> {
        decode_packets(data, allow_compressed)
    }

    /// Append items from a notification (op=5); V2 gifts can contain a batch.
    fn parse_notification(body: &[u8], items: &mut Vec<DanmuItem>) {
        command::parse_notification(body, items);
    }

    #[cfg(test)]
    fn parse_danmu_msg(json: &Value) -> Option<crate::danmaku::DanmuMessage> {
        command::parse_danmu_msg(json)
    }
}

impl DanmuProtocolFactory for BilibiliDanmuProtocol {
    type Protocol = Self;

    fn platform(&self) -> &str {
        "bilibili"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        capture_group_1_owned(&URL_REGEX, url)
    }

    fn create_protocol(&self) -> Self::Protocol {
        Self {
            client: self.client.clone(),
            cookies: self.cookies.clone(),
            uid: None,
            connection_cookies: None,
            resolved_session: None,
        }
    }
}

impl DanmuProtocol for BilibiliDanmuProtocol {
    fn use_http_client(&mut self, client: Client) {
        self.client = client;
    }

    async fn websocket_url(&mut self, room_id: &str) -> Result<String> {
        let real_room_id = self.get_real_room_id(room_id).await?;
        let (ws_url, token) = self.get_danmu_info(real_room_id).await?;
        self.resolved_session = Some((room_id.to_owned(), real_room_id, token));

        Ok(ws_url)
    }

    fn headers(&self, _room_id: &str) -> HeaderMap {
        ws_headers_origin_referer_ua(
            "https://live.bilibili.com",
            "https://live.bilibili.com",
            DEFAULT_UA,
        )
    }

    fn cookies(&self) -> Option<String> {
        let fallback_buvid3 = generate_fake_buvid3();
        Some(Self::build_cookie_header(
            self.cookies.as_deref(),
            &fallback_buvid3,
        ))
    }

    fn send_cookie_header(&self) -> bool {
        false
    }

    fn normalize_cookies(&self, cookies: &str) -> String {
        let fallback_buvid3 = generate_fake_buvid3();
        Self::build_cookie_header(Some(cookies), &fallback_buvid3)
    }

    fn configure_connection(
        &mut self,
        cookies: Option<&str>,
        _extras: Option<&HashMap<String, String>>,
    ) {
        self.uid = cookies
            .and_then(|cookies| extract_cookie_value(cookies, "DedeUserID"))
            .and_then(|v| v.parse::<u64>().ok());
        self.connection_cookies = cookies.map(ToString::to_string);
        self.resolved_session = None;
    }

    async fn handshake_messages(&mut self, room_id: &str) -> Result<Vec<Message>> {
        let (real_room_id, token) = match self.resolved_session.take() {
            Some((cached_room, real_id, tok)) if cached_room == room_id => (real_id, tok),
            _ => {
                let real_id = self.get_real_room_id(room_id).await?;
                let (_ws_url, tok) = self.get_danmu_info(real_id).await?;
                (real_id, tok)
            }
        };

        let auth_packet = self.build_auth_packet(real_room_id, &token);

        Ok(vec![Message::Binary(auth_packet)])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        Some(Message::Binary(Bytes::from_static(HEARTBEAT)))
    }

    fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(HEARTBEAT_INTERVAL_SECS)
    }

    async fn decode_message(
        &mut self,
        message: &Message,
        _room_id: &str,
    ) -> Result<DanmuProtocolOutput> {
        match message {
            Message::Binary(data) => {
                let packets = Self::decode_packets(data, true)?;
                let mut items = Vec::new();

                for packet in packets {
                    match packet.operation {
                        op::NOTIFICATION => {
                            Self::parse_notification(&packet.body, &mut items);
                        }
                        op::AUTH_REPLY => {
                            debug!("Bilibili auth reply received");
                        }
                        _ => {}
                    }
                }

                Ok(items.into())
            }
            _ => Ok(DanmuProtocolOutput::default()),
        }
    }
}

// Re-export the provider type
pub type BilibiliDanmuProvider = WebSocketDanmuProvider<BilibiliDanmuProtocol>;

/// Create a new Bilibili danmu provider.
pub fn create_bilibili_danmu_provider() -> BilibiliDanmuProvider {
    WebSocketDanmuProvider::with_factory(BilibiliDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use byteorder::{BigEndian, ByteOrder};
    use prost::Message as ProstMessage;

    use super::command::SendGiftV2;
    use super::packet::ver;
    use crate::danmaku::{ConnectionConfig, DanmuControlEvent};

    use super::*;

    fn parse_single_notification(body: &[u8]) -> Option<DanmuItem> {
        let mut items = Vec::new();
        BilibiliDanmuProtocol::parse_notification(body, &mut items);
        assert!(items.len() <= 1);
        items.pop()
    }

    // Independently encoded wire fixture: two gifts, a uid above JS's safe
    // integer range, second/millisecond timestamps, and unknown fields in
    // both the envelope and first gift. Keep literal to detect schema drift.
    const GIFT_V2_PB: &str = "CIGAgICAgIAQEghHaWZ0VXNlclIbEgZSb2NrZXQYBShkQgRnb2xkUIDiz6oGmAZ7UhwSBkZsb3dlchgCKMgBQgZzaWx2ZXJQ+9CV/7wxWAGiBgd1bmtub3du";

    fn gift_v2_body(pb: &str) -> Vec<u8> {
        // cmd first exercises the fast command filter as well as dispatch.
        format!(r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":"{pb}"}}}}"#).into_bytes()
    }

    fn gift_v2_fixture() -> SendGiftV2 {
        SendGiftV2::decode(STANDARD.decode(GIFT_V2_PB).unwrap().as_slice()).unwrap()
    }

    fn parse_gift_v2_batch(batch: &SendGiftV2) -> Vec<DanmuItem> {
        let body = gift_v2_body(&STANDARD.encode(batch.encode_to_vec()));
        let mut items = Vec::new();
        BilibiliDanmuProtocol::parse_notification(&body, &mut items);
        items
    }

    #[test]
    #[ignore = "manual decoder microbenchmark; run with --release --ignored --nocapture"]
    fn benchmark_send_gift_v2_notifications() {
        use std::hint::black_box;
        use std::time::Instant;

        for gift_count in [1, 8, 32] {
            let mut batch = gift_v2_fixture();
            batch.gift_list = vec![batch.gift_list[0].clone(); gift_count];
            let body = serde_json::to_vec(&serde_json::json!({
                "cmd": "SEND_GIFT_V2",
                "danmu": {"area": 0},
                "data": {"dmscore": 476, "pb": STANDARD.encode(batch.encode_to_vec())}
            }))
            .unwrap();
            let iterations = 100_000 / gift_count;
            let mut samples = Vec::new();
            for _ in 0..5 {
                let start = Instant::now();
                for _ in 0..iterations {
                    let mut items = Vec::new();
                    BilibiliDanmuProtocol::parse_notification(black_box(&body), &mut items);
                    black_box(items);
                }
                samples.push(start.elapsed().as_nanos() / iterations as u128);
            }
            samples.sort_unstable();
            eprintln!(
                "{gift_count} gifts/notification: median {} ns/notification",
                samples[2]
            );
        }
    }

    fn assert_v2_gifts(items: &[DanmuItem]) {
        assert_eq!(items.len(), 2);
        for (item, name, count, price, timestamp) in [
            (&items[0], "Rocket", 5, 100, 1_700_000_000_000_i64),
            (&items[1], "Flower", 2, 200, 1_700_000_000_123_i64),
        ] {
            let DanmuItem::Message(msg) = item else {
                panic!("expected gift message");
            };
            assert_eq!(msg.message_type, crate::danmaku::message::DanmuType::Gift);
            assert_eq!(msg.user_id, "9007199254740993");
            assert_eq!(msg.username, "GiftUser");
            assert_eq!(msg.content, format!("赠送 {name} x{count}"));
            assert_eq!(msg.timestamp.as_millisecond(), timestamp);
            let meta = msg.metadata.as_ref().unwrap();
            assert_eq!(meta["gift_name"], name);
            assert_eq!(meta["gift_count"], count);
            assert_eq!(meta["price"], price);
        }
    }

    #[tokio::test]
    async fn test_send_gift_v2_decodes_batches_in_raw_and_compressed_frames() {
        use std::io::Write;

        let notification = build_packet(&gift_v2_body(GIFT_V2_PB), op::NOTIFICATION);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&notification).unwrap();
        let mut compressed = build_packet(&encoder.finish().unwrap(), op::NOTIFICATION);
        BigEndian::write_u16(&mut compressed[6..8], ver::ZLIB);
        let mut protocol = BilibiliDanmuProtocol::default();
        for frame in [notification, compressed] {
            let output = protocol
                .decode_message(&Message::Binary(frame.into()), "1")
                .await
                .unwrap();
            let (items, outbound) = output.into_parts();
            assert!(outbound.is_empty());
            assert_v2_gifts(&items);
        }
    }

    #[test]
    fn test_send_gift_v2_accepts_reordered_json_and_command_suffix() {
        let body = format!(r#"{{"data":{{"pb":"{GIFT_V2_PB}"}},"cmd":"SEND_GIFT_V2:1"}}"#);
        let mut items = Vec::new();
        BilibiliDanmuProtocol::parse_notification(body.as_bytes(), &mut items);
        assert_v2_gifts(&items);
    }

    #[test]
    fn test_send_gift_v2_accepts_escaped_strings_and_duplicate_payload_fields() {
        let escaped_pb = GIFT_V2_PB.replacen('C', r"\u0043", 1);
        for body in [
            format!(r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":"{escaped_pb}"}}}}"#),
            format!(r#"{{"cmd":"SEND_GIFT_\u00562","data":{{"pb":"{GIFT_V2_PB}"}}}}"#),
            format!(r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":null,"pb":"{GIFT_V2_PB}"}}}}"#),
            format!(r#"{{"cmd":"SEND_GIFT_V2","data":null,"data":{{"pb":"{GIFT_V2_PB}"}}}}"#),
        ] {
            let mut items = Vec::new();
            BilibiliDanmuProtocol::parse_notification(body.as_bytes(), &mut items);
            assert_v2_gifts(&items);
        }
    }

    #[test]
    fn test_send_gift_v2_prefix_does_not_override_last_command_or_accept_invalid_json() {
        let body = format!(
            r#"{{"cmd":"SEND_GIFT_V2","data":{{"pb":"{GIFT_V2_PB}","title":"Changed"}},"cmd":"ROOM_CHANGE"}}"#
        );
        let item = parse_single_notification(body.as_bytes()).expect("room change");
        assert!(matches!(
            item,
            DanmuItem::Control(DanmuControlEvent::RoomInfoChanged { title: Some(title), .. })
                if title == "Changed"
        ));

        let mut malformed = gift_v2_body(GIFT_V2_PB);
        malformed.extend_from_slice(b" trailing garbage");
        assert!(parse_single_notification(&malformed).is_none());
    }

    #[test]
    fn test_send_gift_v2_preserves_anonymous_sender_and_skips_unknown_fields() {
        // Independent wire fixture with uid=0, sender_uinfo.anon, and unknown
        // varint/fixed64/length-delimited/fixed32 fields in the batch and gift.
        // Mirrors the anonymous-sender case in BililiveRecorder commit a27640a.
        let pb = "CAASDOWMv+WQjeeUqOaIt1I6EgZGbG93ZXIYAihkQgRnb2xkUMTo6sUGwAz///////////8ByQzvzauJZ0UjAdIMA/8AgN0M776t3lgBelcIABIQCgzljL/lkI3nlKjmiLcgAUpBCAESCWFub25fdGVzdBoCdjEiLEFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE9KAHgEgHpEgAAAAAAAAAA8hIA/RIAAAAA";
        let item = parse_single_notification(&gift_v2_body(pb)).expect("anonymous gift");
        let DanmuItem::Message(msg) = item else {
            panic!("expected gift");
        };
        assert_eq!(msg.message_type, crate::danmaku::message::DanmuType::Gift);
        assert_eq!(msg.user_id, "0");
        assert_eq!(msg.username, "匿名用户");
        assert_eq!(msg.content, "赠送 Flower x2");
        assert_eq!(msg.timestamp.as_millisecond(), 1_757_066_308_000);
        let meta = msg.metadata.unwrap();
        assert_eq!(meta["gift_count"], 2);
        assert_eq!(meta["price"], 100);
    }

    #[test]
    fn test_send_gift_v2_records_sender_identity_as_delivered() {
        for (uid, uname) in [(0, "匿名用户"), (-1, "TestUser"), (42, "")] {
            let mut batch = gift_v2_fixture();
            batch.uid = uid;
            batch.uname = uname.to_string();
            let items = parse_gift_v2_batch(&batch);
            assert_eq!(items.len(), 2);
            for item in items {
                let DanmuItem::Message(msg) = item else {
                    panic!("expected gift");
                };
                assert_eq!(msg.user_id, uid.to_string());
                assert_eq!(msg.username, uname);
            }
        }
    }

    #[tokio::test]
    async fn test_send_gift_v2_malformed_payloads_do_not_drop_following_notifications() {
        let mut protocol = BilibiliDanmuProtocol::default();
        for data in [
            serde_json::json!({}),
            serde_json::json!({"pb": null}),
            serde_json::json!({"pb": 42}),
            serde_json::json!({"pb": ""}),
            serde_json::json!({"pb": "!invalid!"}),
            // Truncated varint, truncated nested message, wrong wire type.
            serde_json::json!({"pb": STANDARD.encode([0x08, 0x80])}),
            serde_json::json!({"pb": STANDARD.encode([0x52, 0x05, 0x12])}),
            serde_json::json!({"pb": STANDARD.encode([0x0a, 0x00])}),
        ] {
            let body =
                serde_json::to_vec(&serde_json::json!({"cmd": "SEND_GIFT_V2", "data": data}))
                    .unwrap();
            let mut frame = build_packet(&body, op::NOTIFICATION);
            frame.extend(build_packet(&gift_v2_body(GIFT_V2_PB), op::NOTIFICATION));
            let output = protocol
                .decode_message(&Message::Binary(frame.into()), "1")
                .await
                .unwrap();
            let (items, outbound) = output.into_parts();
            assert!(outbound.is_empty());
            assert_v2_gifts(&items);
        }
    }

    #[test]
    fn test_send_gift_v2_disabled_and_empty_batches_emit_nothing() {
        let mut batch = gift_v2_fixture();
        batch.switch = false;
        assert!(parse_gift_v2_batch(&batch).is_empty());
        batch.switch = true;
        batch.gift_list.clear();
        assert!(parse_gift_v2_batch(&batch).is_empty());
        assert!(parse_gift_v2_batch(&SendGiftV2::default()).is_empty());
    }

    #[test]
    fn test_send_gift_v2_invalid_gifts_do_not_hide_valid_siblings() {
        let fixture = gift_v2_fixture();
        for (count, price, coin_type, name) in [
            (-1, 100, "gold", "Rocket"),
            (i64::MAX, 100, "gold", "Rocket"),
            (0, 100, "gold", "Rocket"),
            (5, -1, "gold", "Rocket"),
            (5, 100, "unknown", "Rocket"),
            (5, 100, "gold", ""),
        ] {
            let mut batch = fixture.clone();
            batch.gift_list[0].num = count;
            batch.gift_list[0].price = price;
            batch.gift_list[0].coin_type = coin_type.to_string();
            batch.gift_list[0].gift_name = name.to_string();
            let items = parse_gift_v2_batch(&batch);
            assert_eq!(items.len(), 1);
            let DanmuItem::Message(msg) = &items[0] else {
                panic!("expected gift");
            };
            assert_eq!(msg.content, "赠送 Flower x2");
        }
    }

    #[test]
    fn test_send_gift_v2_invalid_timestamps_fall_back_to_reception_time() {
        for timestamp in [0, -1, i64::MIN, i64::MAX] {
            let mut batch = gift_v2_fixture();
            batch.gift_list[0].timestamp = timestamp;
            let before = jiff::Timestamp::now();
            let items = parse_gift_v2_batch(&batch);
            let after = jiff::Timestamp::now();
            assert_eq!(items.len(), 2);
            let DanmuItem::Message(msg) = &items[0] else {
                panic!("expected gift");
            };
            assert!(msg.timestamp >= before && msg.timestamp <= after);
        }
    }

    #[test]
    fn test_build_packet() {
        let body = b"test";
        let packet = build_packet(body, op::AUTH);

        assert_eq!(BigEndian::read_u32(&packet[0..4]), 20); // 16 + 4
        assert_eq!(BigEndian::read_u16(&packet[4..6]), 16);
        assert_eq!(BigEndian::read_u16(&packet[6..8]), 1);
        assert_eq!(BigEndian::read_u32(&packet[8..12]), op::AUTH);
        assert_eq!(BigEndian::read_u32(&packet[12..16]), 1);
        assert_eq!(&packet[16..], body);
    }

    #[test]
    fn decode_packets_rejects_lengths_shorter_than_the_header() {
        for packet_len in [0_u32, 15] {
            let mut packet = vec![0_u8; 16];
            BigEndian::write_u32(&mut packet[0..4], packet_len);
            BigEndian::write_u16(&mut packet[4..6], 16);

            assert!(
                BilibiliDanmuProtocol::decode_packets(&Bytes::from(packet), true)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn decode_packets_rejects_lengths_beyond_the_frame() {
        let mut packet = vec![0_u8; 16];
        BigEndian::write_u32(&mut packet[0..4], 17);
        BigEndian::write_u16(&mut packet[4..6], 16);

        assert!(
            BilibiliDanmuProtocol::decode_packets(&Bytes::from(packet), true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn decode_packets_rejects_short_header_len_nested_compression_and_decompression_bombs() {
        use crate::danmaku::websocket::MAX_DECOMPRESSED_FRAME_BYTES;
        use std::io::Write;

        // 1. header_len < 16 with packet_len >= 16 must not slice &[16..header_len] and panic.
        let mut bad_header = vec![0_u8; 20];
        BigEndian::write_u32(&mut bad_header[0..4], 20);
        BigEndian::write_u16(&mut bad_header[4..6], 8);
        assert!(
            BilibiliDanmuProtocol::decode_packets(&Bytes::from(bad_header), true)
                .unwrap()
                .is_empty()
        );

        // 2. Nested Zlib-inside-Zlib compressed frame must be rejected.
        let inner_raw = build_packet(b"{}", op::NOTIFICATION);
        let mut inner_enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        inner_enc.write_all(&inner_raw).unwrap();
        let mut inner_zlib_pkt = build_packet(&inner_enc.finish().unwrap(), op::NOTIFICATION);
        BigEndian::write_u16(&mut inner_zlib_pkt[6..8], ver::ZLIB);

        let mut outer_enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        outer_enc.write_all(&inner_zlib_pkt).unwrap();
        let mut nested_pkt = build_packet(&outer_enc.finish().unwrap(), op::NOTIFICATION);
        BigEndian::write_u16(&mut nested_pkt[6..8], ver::ZLIB);

        let err = BilibiliDanmuProtocol::decode_packets(&Bytes::from(nested_pkt), true)
            .expect_err("nested compressed packets must fail");
        assert!(matches!(err, DanmakuError::Protocol(_)), "{err:?}");

        // 3. Oversized Zlib and Brotli payloads exceeding MAX_DECOMPRESSED_FRAME_BYTES must error.
        let oversized = vec![0_u8; MAX_DECOMPRESSED_FRAME_BYTES + 1024];

        let mut zlib_bomb_enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        zlib_bomb_enc.write_all(&oversized).unwrap();
        let mut zlib_bomb_pkt = build_packet(&zlib_bomb_enc.finish().unwrap(), op::NOTIFICATION);
        BigEndian::write_u16(&mut zlib_bomb_pkt[6..8], ver::ZLIB);
        assert!(BilibiliDanmuProtocol::decode_packets(&Bytes::from(zlib_bomb_pkt), true).is_err());

        let mut brotli_bomb = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut brotli_bomb, 4096, 1, 20);
            writer.write_all(&oversized).unwrap();
        }
        let mut brotli_bomb_pkt = build_packet(&brotli_bomb, op::NOTIFICATION);
        BigEndian::write_u16(&mut brotli_bomb_pkt[6..8], ver::BROTLI);
        assert!(
            BilibiliDanmuProtocol::decode_packets(&Bytes::from(brotli_bomb_pkt), true).is_err()
        );
    }

    #[tokio::test]
    async fn handshake_messages_consumes_cached_session_without_network_calls() {
        let mut protocol = BilibiliDanmuProtocol::default();
        protocol.configure_connection(Some("DedeUserID=99; SESSDATA=ok"), None);
        protocol.resolved_session = Some(("short6".to_string(), 60006, "cached-tok".to_string()));

        let msgs = protocol.handshake_messages("short6").await.unwrap();
        assert_eq!(msgs.len(), 1);
        let Message::Binary(packet) = &msgs[0] else {
            panic!("expected binary auth packet");
        };
        let auth: Value = serde_json::from_slice(&packet[16..]).unwrap();
        assert_eq!(auth["roomid"], 60006);
        assert_eq!(auth["key"], "cached-tok");
        assert_eq!(auth["uid"], 99);
        assert!(protocol.resolved_session.is_none());
    }

    #[test]
    fn test_auth_packet_uses_uid_from_cookies() {
        let mut protocol = BilibiliDanmuProtocol::default();
        protocol.configure_connection(Some("DedeUserID=42; SESSDATA=abc"), None);

        let packet = protocol.build_auth_packet(123, "token");
        let json: serde_json::Value = serde_json::from_slice(&packet[16..]).unwrap();
        assert_eq!(json.get("uid").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(json.get("roomid").and_then(|v| v.as_u64()), Some(123));
    }

    #[test]
    fn test_cookie_buvid3_uses_provided_value() {
        let cookies = "SESSDATA=abc; buvid3=provided; bili_jct=xyz";
        let merged = BilibiliDanmuProtocol::build_cookie_header(Some(cookies), "fallback");
        assert!(merged.contains("buvid3=provided"));
        assert!(!merged.contains("buvid3=fallback"));
        assert!(!merged.contains("buvid3=provided; buvid3="));
        assert!(merged.contains("SESSDATA=abc"));
        assert!(merged.contains("bili_jct=xyz"));
    }

    #[test]
    fn test_cookie_adds_fallback_buvid3_when_missing() {
        let cookies = "SESSDATA=abc; bili_jct=xyz";
        let merged = BilibiliDanmuProtocol::build_cookie_header(Some(cookies), "fallback");
        assert!(merged.starts_with("buvid3=fallback; "));
        assert!(merged.contains("SESSDATA=abc"));
        assert!(merged.contains("bili_jct=xyz"));
    }

    #[test]
    fn test_parse_danmu_msg() {
        let json = serde_json::json!({
            "cmd": "DANMU_MSG",
            "info": [
                [0, 1, 25, 16777215, 0, 0, 0, "", 0, 0, 0, "", 0, "{}", "{}", {"extra": "{}"}],
                "Hello World",
                [12345, "TestUser", 0, 0, 0, 0, 0, ""]
            ]
        });

        let danmu = BilibiliDanmuProtocol::parse_danmu_msg(&json);
        assert!(danmu.is_some());

        let msg = danmu.unwrap();
        assert_eq!(msg.content, "Hello World");
        assert_eq!(msg.username, "TestUser");
        assert_eq!(msg.user_id, "12345");
    }

    #[test]
    fn test_parse_send_gift_emits_gift_message() {
        let json = serde_json::json!({
            "cmd": "SEND_GIFT",
            "data": {
                "uname": "GiftUser",
                "uid": 42,
                "giftName": "Rocket",
                "num": 5,
                "price": 100,
                "timestamp": 1700000000123_i64
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse SEND_GIFT");

        match item {
            DanmuItem::Message(msg) => {
                assert_eq!(msg.message_type, crate::danmaku::message::DanmuType::Gift);
                assert_eq!(msg.user_id, "42");
                assert_eq!(msg.username, "GiftUser");
                assert_eq!(msg.content, "赠送 Rocket x5");
                let meta = msg.metadata.expect("gift metadata");
                assert_eq!(meta.get("price").unwrap(), 100);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_super_chat_emits_super_chat_message() {
        let json = serde_json::json!({
            "cmd": "SUPER_CHAT_MESSAGE",
            "data": {
                "uid": 99,
                "price": 30,
                "time": 60,
                "ts": 1700000000456_i64,
                "message": "Hello",
                "user_info": {
                    "uname": "SCUser"
                }
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse SUPER_CHAT_MESSAGE");

        match item {
            DanmuItem::Message(msg) => {
                assert_eq!(
                    msg.message_type,
                    crate::danmaku::message::DanmuType::SuperChat
                );
                assert_eq!(msg.user_id, "99");
                assert_eq!(msg.username, "SCUser");
                assert_eq!(msg.content, "Hello");
                let meta = msg.metadata.expect("super chat metadata");
                assert_eq!(meta.get("price").unwrap(), 30);
                assert_eq!(meta.get("keep_time").unwrap(), 60);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_room_change_emits_control() {
        let json = serde_json::json!({
            "cmd": "ROOM_CHANGE",
            "data": {
                "title": "New Stream Title",
                "area_name": "Some Area",
                "parent_area_name": "Some Parent"
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse ROOM_CHANGE");

        match item {
            DanmuItem::Control(DanmuControlEvent::RoomInfoChanged {
                title,
                category,
                parent_category,
            }) => {
                assert_eq!(title.as_deref(), Some("New Stream Title"));
                assert_eq!(category.as_deref(), Some("Some Area"));
                assert_eq!(parent_category.as_deref(), Some("Some Parent"));
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_room_lock_emits_stream_closed() {
        let json = serde_json::json!({
            "cmd": "ROOM_LOCK",
            "data": {
                "message": "room locked"
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse ROOM_LOCK");

        match item {
            DanmuItem::Control(DanmuControlEvent::StreamClosed { message, action }) => {
                assert_eq!(message.as_deref(), Some("room locked"));
                assert_eq!(action, None);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_parse_cut_off_emits_stream_closed() {
        let json = serde_json::json!({
            "cmd": "CUT_OFF",
            "data": {
                "msg": "cut off"
            }
        });

        let body = serde_json::to_vec(&json).unwrap();
        let item = parse_single_notification(&body).expect("should parse CUT_OFF");

        match item {
            DanmuItem::Control(DanmuControlEvent::StreamClosed { message, action }) => {
                assert_eq!(message.as_deref(), Some("cut off"));
                assert_eq!(action, None);
            }
            other => panic!("Unexpected item: {other:?}"),
        }
    }

    /// Real integration test - connects to an actual Bilibili live room
    /// Run with: cargo test --package platforms-parser bilibili::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_real_connection() {
        use crate::danmaku::provider::DanmuProvider;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();

        let provider = create_bilibili_danmu_provider();
        let room_id = "1721766859";

        println!("Connecting to Bilibili room: {}", room_id);
        let mut items = match provider.connect(room_id, ConnectionConfig::default()).await {
            Ok(stream) => stream.items,
            Err(e) => {
                eprintln!("Failed to connect: {}", e);
                return;
            }
        };

        // Receive messages for 60 seconds
        let start = std::time::Instant::now();
        let mut message_count = 0;

        while start.elapsed() < Duration::from_secs(60) {
            match tokio::time::timeout(Duration::from_millis(500), items.recv()).await {
                Ok(Some(item)) => match item {
                    crate::danmaku::DanmuItem::Message(msg) => {
                        println!("[{:?}] {}: {}", msg.message_type, msg.username, msg.content);
                        message_count += 1;
                    }
                    crate::danmaku::DanmuItem::Control(control) => {
                        println!("[control] {:?}", control);
                    }
                },
                Ok(None) => {
                    println!("Stream closed by provider");
                    break;
                }
                Err(_) => {
                    // No message within the window; keep waiting until the 60s
                    // budget is spent.
                }
            }
        }

        println!("Received {} messages", message_count);
    }
}
