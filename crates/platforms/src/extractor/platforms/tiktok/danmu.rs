//! TikTok LIVE danmu (chat) provider.
//!
//! Connects directly to the webcast IM WebSocket used by the web player:
//! `wss://webcast-ws.tiktok.com/webcast/im/ws_proxy/ws_reuse_supplement/`.
//! The server only requires a `ttwid` session cookie on the upgrade request;
//! no `X-Bogus` / `X-Dynosaur` signature or third-party sign server is needed.
//! Frames are protobuf `PushFrame`s whose `msg` / `im_enter_room_resp`
//! payloads decode to a `Response` carrying `WebcastChatMessage`,
//! `WebcastGiftMessage`, `WebcastControlMessage`, and similar messages.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::time::Duration;

use bytes::Bytes;
use chrono::{TimeZone, Utc};
use flate2::read::GzDecoder;
use prost::Message as ProstMessage;
use reqwest::Client;
use tokio_tungstenite::tungstenite::http::{HeaderMap, HeaderValue, header};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};
use url::form_urlencoded;

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{
    DanmuProtocol, DanmuProtocolFactory, DanmuProtocolOutput, WebSocketDanmuProvider,
};
use crate::danmaku::{DanmuControlEvent, DanmuItem, DanmuMessage};
use crate::extractor::default::{DEFAULT_UA, default_client};
use crate::extractor::platforms::tiktok::tiktok_proto as proto;
use crate::extractor::platforms::tiktok::utils::{
    TIKTOK_WEB_URL, WEBCAST_AID, WEBCAST_APP_NAME, ensure_global_ttwid,
};
use crate::extractor::platforms::tiktok::{TikTok, URL_REGEX};
use crate::extractor::utils::capture_group_1_owned;

/// Global webcast WebSocket edge. Regional edges (`webcast-ws.eu.tiktok.com`,
/// `webcast-ws.us.tiktok.com`) accept the same request; the global host works
/// for rooms in every region observed.
const WS_HOST: &str = "wss://webcast-ws.tiktok.com";
const WS_PATH: &str = "/webcast/im/ws_proxy/ws_reuse_supplement/";

/// Heartbeat cadence requested through `heartbeat_duration` on the socket URL.
const HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// Number of recent message IDs remembered to drop server re-deliveries.
const SEEN_MESSAGE_CAPACITY: usize = 2048;

const PAYLOAD_TYPE_MSG: &str = "msg";
const PAYLOAD_TYPE_ENTER_ROOM_RESP: &str = "im_enter_room_resp";
const PAYLOAD_TYPE_HEARTBEAT: &str = "hb";
const PAYLOAD_TYPE_ENTER_ROOM: &str = "im_enter_room";
const PAYLOAD_TYPE_ACK: &str = "ack";
const PAYLOAD_TYPE_CLOSE: &str = "close";

/// `WebcastControlMessage.action` values that end the broadcast.
const CONTROL_ACTION_STREAM_ENDED: i32 = 3;
const CONTROL_ACTION_STREAM_SUSPENDED: i32 = 4;

/// Task-owned TikTok webcast IM protocol state.
pub struct TikTokDanmuProtocol {
    client: Client,
    /// Cookies supplied by the caller (merged with the ttwid on the upgrade).
    cookies: Option<String>,
    /// `ttwid` used for this connection attempt.
    ttwid: Option<String>,
    /// Numeric webcast room ID resolved for the current attempt.
    room_id: Option<String>,
    seen: VecDeque<i64>,
    seen_set: HashSet<i64>,
}

impl Default for TikTokDanmuProtocol {
    fn default() -> Self {
        Self {
            client: default_client(),
            cookies: None,
            ttwid: None,
            room_id: None,
            seen: VecDeque::with_capacity(SEEN_MESSAGE_CAPACITY),
            seen_set: HashSet::with_capacity(SEEN_MESSAGE_CAPACITY),
        }
    }
}

impl TikTokDanmuProtocol {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            cookies: Some(cookies.into()),
            ..Self::default()
        }
    }

    fn numeric_room_id(room_id: &str) -> bool {
        !room_id.is_empty() && room_id.bytes().all(|b| b.is_ascii_digit())
    }

    /// Resolves a `@handle` to the numeric webcast room ID via the JSON API.
    async fn resolve_room_id(&self, unique_id: &str) -> Result<String> {
        let unique_id = unique_id.trim_start_matches('@');
        let extractor = TikTok::new(
            format!("{TIKTOK_WEB_URL}/@{unique_id}/live"),
            self.client.clone(),
            self.cookies.clone(),
            None,
        );
        let info = extractor
            .fetch_api_live_room(unique_id)
            .await
            .map_err(|e| DanmakuError::connection(format!("resolve TikTok room id: {e}")))?;
        if info.user.room_id.is_empty() {
            return Err(DanmakuError::connection(format!(
                "TikTok user @{unique_id} has no room id"
            )));
        }
        Ok(info.user.room_id)
    }

    fn build_websocket_url(room_id: &str) -> String {
        let browser_version = DEFAULT_UA.strip_prefix("Mozilla/").unwrap_or(DEFAULT_UA);
        let mut query = form_urlencoded::Serializer::new(String::new());
        // Mirrors the web player's `ws_direct` connection. `version_code`
        // appears twice because the player appends `270000` after the base
        // parameter set; the server accepts the pair.
        for (key, value) in [
            ("version_code", "180800"),
            ("device_platform", "web"),
            ("cookie_enabled", "true"),
            ("screen_width", "1920"),
            ("screen_height", "1080"),
            ("browser_language", "en-US"),
            ("browser_platform", "Win32"),
            ("browser_name", "Mozilla"),
            ("browser_version", browser_version),
            ("browser_online", "true"),
            ("tz_name", "UTC"),
            ("app_name", WEBCAST_APP_NAME),
            ("sup_ws_ds_opt", "1"),
            ("update_version_code", "2.0.0"),
            ("compress", "gzip"),
            ("webcast_language", "en"),
            ("ws_direct", "1"),
            ("aid", WEBCAST_AID),
            ("live_id", "12"),
            ("version_code", "270000"),
            ("app_language", "en"),
            ("client_enter", "1"),
            ("room_id", room_id),
            ("identity", "audience"),
            // 0 so reconnects do not replay comments already recorded.
            ("history_comment_count", "0"),
            ("last_rtt", "0"),
            ("heartbeat_duration", "10000"),
            ("resp_content_type", "protobuf"),
            ("did_rule", "3"),
        ] {
            query.append_pair(key, value);
        }
        format!("{WS_HOST}{WS_PATH}?{}", query.finish())
    }

    fn push_frame(payload_type: &str, payload: Vec<u8>, log_id: u64) -> Message {
        let frame = proto::PushFrame {
            log_id,
            payload_encoding: "pb".to_string(),
            payload_type: payload_type.to_string(),
            payload,
            ..Default::default()
        };
        Message::Binary(Bytes::from(frame.encode_to_vec()))
    }

    fn heartbeat_frame(room_id: i64) -> Message {
        let hb = proto::HeartBeat {
            room_id,
            ..Default::default()
        };
        Self::push_frame(PAYLOAD_TYPE_HEARTBEAT, hb.encode_to_vec(), 0)
    }

    fn enter_room_frame(room_id: i64) -> Message {
        let enter = proto::EnterRoom {
            room_id,
            live_id: 12,
            identity: "audience".to_string(),
            filter_welcome_msg: "0".to_string(),
            enter_unique_id: (rand::random::<u64>() >> 1).max(1) as i64,
            ..Default::default()
        };
        Self::push_frame(PAYLOAD_TYPE_ENTER_ROOM, enter.encode_to_vec(), 0)
    }

    fn ack_frame(log_id: u64, internal_ext: &str) -> Message {
        Self::push_frame(PAYLOAD_TYPE_ACK, internal_ext.as_bytes().to_vec(), log_id)
    }

    fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>> {
        let mut decoder = GzDecoder::new(data);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| DanmakuError::protocol(format!("Failed to decompress gzip: {e}")))?;
        Ok(out)
    }

    fn header_value<'a>(frame: &'a proto::PushFrame, key: &str) -> Option<&'a str> {
        frame
            .headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
    }

    fn decode_push_frame(data: &[u8]) -> Result<(proto::PushFrame, Option<proto::Response>)> {
        let frame = proto::PushFrame::decode(data)?;
        if frame.payload_type != PAYLOAD_TYPE_MSG
            && frame.payload_type != PAYLOAD_TYPE_ENTER_ROOM_RESP
        {
            return Ok((frame, None));
        }
        let gzip = Self::header_value(&frame, "compress_type") == Some("gzip")
            || frame.payload.starts_with(&[0x1f, 0x8b]);
        let payload = if gzip {
            Self::decompress_gzip(&frame.payload)?
        } else {
            frame.payload.clone()
        };
        let response = proto::Response::decode(payload.as_slice())?;
        Ok((frame, Some(response)))
    }

    /// Returns `false` when `msg_id` was already delivered on this connection.
    fn mark_seen(&mut self, msg_id: i64) -> bool {
        if msg_id == 0 {
            return true;
        }
        if !self.seen_set.insert(msg_id) {
            return false;
        }
        self.seen.push_back(msg_id);
        while self.seen.len() > SEEN_MESSAGE_CAPACITY {
            if let Some(old) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
        true
    }

    fn timestamp_from_common(common: Option<&proto::Common>) -> chrono::DateTime<Utc> {
        common
            .map(|c| c.create_time)
            .filter(|&t| t > 0)
            .and_then(|t| Utc.timestamp_millis_opt(t).single())
            .unwrap_or_else(Utc::now)
    }

    fn user_parts(user: Option<&proto::User>) -> (String, String) {
        let Some(user) = user else {
            return (String::new(), String::new());
        };
        let username = if user.nickname.is_empty() {
            user.display_id.clone()
        } else {
            user.nickname.clone()
        };
        (user.id.to_string(), username)
    }

    fn parse_chat(payload: &[u8]) -> Result<Option<DanmuMessage>> {
        let chat = proto::ChatMessage::decode(payload)?;
        let content = chat.content.trim();
        if content.is_empty() {
            return Ok(None);
        }
        let (user_id, username) = Self::user_parts(chat.user.as_ref());
        let msg_id = chat.common.as_ref().map_or(0, |c| c.msg_id);
        let mut message = DanmuMessage::chat(msg_id.to_string(), user_id, username, content)
            .with_timestamp(Self::timestamp_from_common(chat.common.as_ref()));
        if let Some(user) = chat.user.as_ref()
            && !user.display_id.is_empty()
        {
            message = message.with_metadata("display_id", serde_json::json!(user.display_id));
        }
        if !chat.emotes.is_empty() {
            let ids: Vec<String> = chat
                .emotes
                .iter()
                .filter_map(|e| e.emote.as_ref().map(|m| m.emote_id.clone()))
                .collect();
            message = message.with_metadata("emotes", serde_json::json!(ids));
        }
        Ok(Some(message))
    }

    fn parse_emote_chat(payload: &[u8]) -> Result<Option<DanmuMessage>> {
        let chat = proto::EmoteChatMessage::decode(payload)?;
        if chat.emote_list.is_empty() {
            return Ok(None);
        }
        let content = chat
            .emote_list
            .iter()
            .map(|e| format!("[emote:{}]", e.emote_id))
            .collect::<Vec<_>>()
            .join("");
        let (user_id, username) = Self::user_parts(chat.user.as_ref());
        let msg_id = chat.common.as_ref().map_or(0, |c| c.msg_id);
        Ok(Some(
            DanmuMessage::chat(msg_id.to_string(), user_id, username, content)
                .with_timestamp(Self::timestamp_from_common(chat.common.as_ref())),
        ))
    }

    fn parse_gift(payload: &[u8]) -> Result<Option<DanmuMessage>> {
        let gift_msg = proto::GiftMessage::decode(payload)?;
        let Some(gift) = gift_msg.gift.as_ref() else {
            return Ok(None);
        };
        // Combo gifts stream one message per tap with a running
        // `repeat_count`; only the final message (`repeat_end == 1`) carries
        // the total, so intermediate ones are dropped to avoid double counting.
        if gift.combo && gift_msg.repeat_end != 1 {
            return Ok(None);
        }
        let count = u32::try_from(gift_msg.repeat_count.max(1)).unwrap_or(1);
        let gift_name = if gift.name.is_empty() {
            format!("gift#{}", gift.id)
        } else {
            gift.name.clone()
        };
        let (user_id, username) = Self::user_parts(gift_msg.user.as_ref());
        let msg_id = gift_msg.common.as_ref().map_or(0, |c| c.msg_id);
        let message = DanmuMessage::gift(msg_id.to_string(), user_id, username, gift_name, count)
            .with_timestamp(Self::timestamp_from_common(gift_msg.common.as_ref()))
            .with_metadata("gift_id", serde_json::json!(gift.id))
            .with_metadata("diamond_count", serde_json::json!(gift.diamond_count));
        Ok(Some(message))
    }

    fn parse_control(payload: &[u8]) -> Result<Option<DanmuControlEvent>> {
        let control = proto::ControlMessage::decode(payload)?;
        if control.action != CONTROL_ACTION_STREAM_ENDED
            && control.action != CONTROL_ACTION_STREAM_SUSPENDED
        {
            return Ok(None);
        }
        let tips = control.tips.trim();
        Ok(Some(DanmuControlEvent::StreamClosed {
            message: (!tips.is_empty()).then(|| tips.to_string()),
            action: u64::try_from(control.action).ok(),
        }))
    }

    fn parse_response_messages(&mut self, messages: &[proto::Message]) -> Vec<DanmuItem> {
        let mut items = Vec::new();
        for message in messages {
            if !self.mark_seen(message.msg_id) {
                continue;
            }
            let parsed = match message.method.as_str() {
                "WebcastChatMessage" => {
                    Self::parse_chat(&message.payload).map(|m| m.map(DanmuItem::Message))
                }
                "WebcastEmoteChatMessage" => {
                    Self::parse_emote_chat(&message.payload).map(|m| m.map(DanmuItem::Message))
                }
                "WebcastGiftMessage" => {
                    Self::parse_gift(&message.payload).map(|m| m.map(DanmuItem::Message))
                }
                "WebcastControlMessage" => {
                    Self::parse_control(&message.payload).map(|c| c.map(DanmuItem::Control))
                }
                _ => Ok(None),
            };
            match parsed {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {}
                Err(e) => {
                    debug!(method = %message.method, error = %e, "skipping undecodable TikTok message")
                }
            }
        }
        items
    }
}

impl DanmuProtocolFactory for TikTokDanmuProtocol {
    type Protocol = Self;

    fn platform(&self) -> &str {
        "tiktok"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    /// Returns the `@handle`; `websocket_url` resolves it to the numeric room
    /// ID when the caller has not already supplied one via extras.
    fn extract_room_id(&self, url: &str) -> Option<String> {
        // Preserve the prefix so numeric handles remain distinct from room IDs.
        capture_group_1_owned(&URL_REGEX, url).map(|handle| format!("@{handle}"))
    }

    fn create_protocol(&self) -> Self::Protocol {
        Self {
            client: self.client.clone(),
            cookies: self.cookies.clone(),
            ..Self::default()
        }
    }
}

impl DanmuProtocol for TikTokDanmuProtocol {
    fn cookies(&self) -> Option<String> {
        self.cookies.clone()
    }

    fn configure_connection(
        &mut self,
        cookies: Option<&str>,
        _extras: Option<&HashMap<String, String>>,
    ) {
        if let Some(c) = cookies.map(str::trim).filter(|c| !c.is_empty()) {
            self.cookies = Some(c.to_string());
        }
        self.room_id = None;
        self.ttwid = None;
        self.seen.clear();
        self.seen_set.clear();
    }

    async fn websocket_url(&mut self, room_id: &str) -> Result<String> {
        let room_id = if Self::numeric_room_id(room_id) {
            room_id.to_string()
        } else {
            self.resolve_room_id(room_id).await?
        };
        // The socket upgrade is rejected (HTTP 200 without upgrade) unless a
        // ttwid cookie accompanies it; register one when the caller's cookies
        // do not already carry it.
        let has_user_ttwid = self
            .cookies
            .as_deref()
            .is_some_and(|c| c.split(';').any(|p| p.trim().starts_with("ttwid=")));
        if !has_user_ttwid {
            let ttwid = ensure_global_ttwid(&self.client)
                .await
                .map_err(|e| DanmakuError::connection(format!("fetch TikTok ttwid: {e}")))?;
            self.ttwid = Some(ttwid);
        }
        let url = Self::build_websocket_url(&room_id);
        self.room_id = Some(room_id);
        Ok(url)
    }

    fn headers(&self, _room_id: &str) -> HeaderMap {
        let mut headers = HeaderMap::with_capacity(3);
        headers.insert(header::ORIGIN, HeaderValue::from_static(TIKTOK_WEB_URL));
        headers.insert(header::USER_AGENT, HeaderValue::from_static(DEFAULT_UA));
        // Caller cookies are merged over this by the connection runner, so a
        // user-provided ttwid still wins.
        if let Some(ttwid) = self.ttwid.as_deref()
            && let Ok(value) = HeaderValue::from_str(&format!("ttwid={ttwid}"))
        {
            headers.insert(header::COOKIE, value);
        }
        headers
    }

    async fn handshake_messages(&mut self, _room_id: &str) -> Result<Vec<Message>> {
        let room_id = self
            .room_id
            .as_deref()
            .and_then(|r| r.parse::<i64>().ok())
            .ok_or_else(|| DanmakuError::protocol("TikTok room id not resolved"))?;
        // Same order as the web player: a heartbeat, then the room join.
        Ok(vec![
            Self::heartbeat_frame(room_id),
            Self::enter_room_frame(room_id),
        ])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        let room_id = self.room_id.as_deref()?.parse::<i64>().ok()?;
        Some(Self::heartbeat_frame(room_id))
    }

    fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(HEARTBEAT_INTERVAL_SECS)
    }

    async fn decode_message(
        &mut self,
        message: &Message,
        _room_id: &str,
    ) -> Result<DanmuProtocolOutput> {
        let Message::Binary(data) = message else {
            return Ok(DanmuProtocolOutput::default());
        };

        let (frame, response) = Self::decode_push_frame(data)?;
        let Some(response) = response else {
            if frame.payload_type == PAYLOAD_TYPE_CLOSE {
                let reason = Self::header_value(&frame, "close_reason").unwrap_or("");
                warn!(reason, "TikTok webcast socket closed by server");
                return Err(DanmakuError::connection(format!(
                    "TikTok webcast socket closed by server: {reason}"
                )));
            }
            return Ok(DanmuProtocolOutput::default());
        };

        let mut outbound = Vec::new();
        if response.need_ack && frame.log_id != 0 {
            outbound.push(Self::ack_frame(frame.log_id, &response.internal_ext));
        }
        let items = self.parse_response_messages(&response.messages);
        Ok(DanmuProtocolOutput::new(items, outbound))
    }
}

/// TikTok danmu provider type alias.
pub type TikTokDanmuProvider = WebSocketDanmuProvider<TikTokDanmuProtocol>;

/// Creates a new TikTok danmu provider.
pub fn create_tiktok_danmu_provider() -> TikTokDanmuProvider {
    WebSocketDanmuProvider::with_factory(TikTokDanmuProtocol::default(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::danmaku::DanmuType;

    fn user(id: i64, nickname: &str, display_id: &str) -> proto::User {
        proto::User {
            id,
            nickname: nickname.to_string(),
            display_id: display_id.to_string(),
            ..Default::default()
        }
    }

    fn common(msg_id: i64, create_time: i64) -> proto::Common {
        proto::Common {
            msg_id,
            create_time,
            ..Default::default()
        }
    }

    fn response_frame(messages: Vec<proto::Message>, need_ack: bool, gzip: bool) -> Vec<u8> {
        let response = proto::Response {
            messages,
            internal_ext: "-".to_string(),
            need_ack,
            cursor: "1_2_3".to_string(),
            ..Default::default()
        };
        let mut payload = response.encode_to_vec();
        let mut headers = Vec::new();
        if gzip {
            use flate2::{Compression, write::GzEncoder};
            use std::io::Write;
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&payload).unwrap();
            payload = encoder.finish().unwrap();
            headers.push(proto::PushHeader {
                key: "compress_type".to_string(),
                value: "gzip".to_string(),
            });
        }
        proto::PushFrame {
            log_id: 42,
            headers,
            payload_encoding: "pb".to_string(),
            payload_type: PAYLOAD_TYPE_MSG.to_string(),
            payload,
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn chat_message(msg_id: i64, content: &str) -> proto::Message {
        let chat = proto::ChatMessage {
            common: Some(common(msg_id, 1_789_308_623_396)),
            user: Some(user(6_935_366_370_146_911_237, "Pili", "pili_villarreal0")),
            content: content.to_string(),
            ..Default::default()
        };
        proto::Message {
            method: "WebcastChatMessage".to_string(),
            payload: chat.encode_to_vec(),
            msg_id,
            ..Default::default()
        }
    }

    #[test]
    fn websocket_url_targets_direct_ws_proxy() {
        let url = TikTokDanmuProtocol::build_websocket_url("7685002745492884246");
        assert!(
            url.starts_with(
                "wss://webcast-ws.tiktok.com/webcast/im/ws_proxy/ws_reuse_supplement/?"
            )
        );
        assert!(url.contains("room_id=7685002745492884246"));
        assert!(url.contains("ws_direct=1"));
        assert!(url.contains("resp_content_type=protobuf"));
        assert!(url.contains("history_comment_count=0"));
        assert!(!url.contains("X-Bogus"));
    }

    #[test]
    fn extract_room_id_returns_handle() {
        let protocol = TikTokDanmuProtocol::default();
        assert!(protocol.supports_url("https://www.tiktok.com/@dj.ibai/live"));
        assert_eq!(
            protocol.extract_room_id("https://www.tiktok.com/@dj.ibai/live"),
            Some("@dj.ibai".to_string())
        );
        assert!(!TikTokDanmuProtocol::numeric_room_id("dj.ibai"));
        assert!(TikTokDanmuProtocol::numeric_room_id("7685002745492884246"));
    }

    #[test]
    fn handshake_and_heartbeat_frames_round_trip() {
        let hb = TikTokDanmuProtocol::heartbeat_frame(7685002745492884246);
        let Message::Binary(bytes) = hb else {
            panic!("binary")
        };
        let frame = proto::PushFrame::decode(bytes.as_ref()).unwrap();
        assert_eq!(frame.payload_type, PAYLOAD_TYPE_HEARTBEAT);
        assert_eq!(frame.payload_encoding, "pb");
        let hb = proto::HeartBeat::decode(frame.payload.as_slice()).unwrap();
        assert_eq!(hb.room_id, 7685002745492884246);

        let enter = TikTokDanmuProtocol::enter_room_frame(7685002745492884246);
        let Message::Binary(bytes) = enter else {
            panic!("binary")
        };
        let frame = proto::PushFrame::decode(bytes.as_ref()).unwrap();
        assert_eq!(frame.payload_type, PAYLOAD_TYPE_ENTER_ROOM);
        let enter = proto::EnterRoom::decode(frame.payload.as_slice()).unwrap();
        assert_eq!(enter.room_id, 7685002745492884246);
        assert_eq!(enter.live_id, 12);
        assert_eq!(enter.identity, "audience");
        assert!(enter.enter_unique_id > 0);
    }

    #[tokio::test]
    async fn decodes_gzip_chat_and_acks() {
        let mut protocol = TikTokDanmuProtocol::default();
        let frame = response_frame(vec![chat_message(1, "hola ✈️")], true, true);
        let output = protocol
            .decode_message(&Message::Binary(Bytes::from(frame)), "x")
            .await
            .unwrap();
        let (items, outbound) = output.into_parts();
        assert_eq!(items.len(), 1);
        let DanmuItem::Message(msg) = &items[0] else {
            panic!("message")
        };
        assert_eq!(msg.content, "hola ✈️");
        assert_eq!(msg.username, "Pili");
        assert_eq!(msg.user_id, "6935366370146911237");
        assert_eq!(msg.timestamp.timestamp_millis(), 1_789_308_623_396);
        assert_eq!(msg.message_type, DanmuType::Chat);

        assert_eq!(outbound.len(), 1);
        let Message::Binary(ack) = &outbound[0] else {
            panic!("binary")
        };
        let ack = proto::PushFrame::decode(ack.as_ref()).unwrap();
        assert_eq!(ack.payload_type, PAYLOAD_TYPE_ACK);
        assert_eq!(ack.log_id, 42);
        assert_eq!(ack.payload, b"-");
    }

    #[tokio::test]
    async fn drops_redelivered_messages_and_skips_ack_when_not_needed() {
        let mut protocol = TikTokDanmuProtocol::default();
        let first = response_frame(
            vec![chat_message(7, "one"), chat_message(8, "two")],
            false,
            false,
        );
        let (items, outbound) = protocol
            .decode_message(&Message::Binary(Bytes::from(first)), "x")
            .await
            .unwrap()
            .into_parts();
        assert_eq!(items.len(), 2);
        assert!(outbound.is_empty());

        let again = response_frame(
            vec![chat_message(7, "one"), chat_message(9, "three")],
            false,
            false,
        );
        let (items, _) = protocol
            .decode_message(&Message::Binary(Bytes::from(again)), "x")
            .await
            .unwrap()
            .into_parts();
        assert_eq!(items.len(), 1);
        let DanmuItem::Message(msg) = &items[0] else {
            panic!("message")
        };
        assert_eq!(msg.content, "three");
    }

    #[test]
    fn gift_combo_only_emits_final_message() {
        let gift = |repeat_count: i32, repeat_end: i32, combo: bool| {
            proto::GiftMessage {
                common: Some(common(1, 0)),
                gift_id: 5655,
                repeat_count,
                repeat_end,
                user: Some(user(1, "Giver", "giver")),
                gift: Some(proto::Gift {
                    id: 5655,
                    name: "Rose".to_string(),
                    combo,
                    diamond_count: 1,
                    ..Default::default()
                }),
                ..Default::default()
            }
            .encode_to_vec()
        };
        assert!(
            TikTokDanmuProtocol::parse_gift(&gift(3, 0, true))
                .unwrap()
                .is_none()
        );
        let final_msg = TikTokDanmuProtocol::parse_gift(&gift(5, 1, true))
            .unwrap()
            .unwrap();
        assert_eq!(final_msg.message_type, DanmuType::Gift);
        assert_eq!(final_msg.content, "赠送 Rose x5");
        assert_eq!(
            final_msg.metadata.as_ref().unwrap().get("gift_count"),
            Some(&serde_json::json!(5))
        );
        let single = TikTokDanmuProtocol::parse_gift(&gift(1, 0, false))
            .unwrap()
            .unwrap();
        assert_eq!(single.content, "赠送 Rose x1");
    }

    #[test]
    fn control_stream_end_maps_to_stream_closed() {
        let control = |action: i32| {
            proto::ControlMessage {
                common: Some(common(1, 0)),
                action,
                tips: "Stream ended".to_string(),
            }
            .encode_to_vec()
        };
        assert!(
            TikTokDanmuProtocol::parse_control(&control(1))
                .unwrap()
                .is_none()
        );
        match TikTokDanmuProtocol::parse_control(&control(3)).unwrap() {
            Some(DanmuControlEvent::StreamClosed { message, action }) => {
                assert_eq!(message.as_deref(), Some("Stream ended"));
                assert_eq!(action, Some(3));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn close_frame_forces_reconnect() {
        let mut protocol = TikTokDanmuProtocol::default();
        let frame = proto::PushFrame {
            payload_type: PAYLOAD_TYPE_CLOSE.to_string(),
            headers: vec![proto::PushHeader {
                key: "close_reason".to_string(),
                value: "disable_web_direct".to_string(),
            }],
            ..Default::default()
        }
        .encode_to_vec();
        let err = protocol
            .decode_message(&Message::Binary(Bytes::from(frame)), "x")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("disable_web_direct"));
    }

    /// Live integration check: joins a currently live room and prints chat.
    /// Run with: cargo test -p platforms-parser tiktok::danmu::tests::test_real_connection -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_real_connection() {
        use crate::danmaku::{ConnectionConfig, DanmuProvider};

        let provider = create_tiktok_danmu_provider();
        let room = std::env::var("TIKTOK_LIVE_HANDLE").unwrap_or_else(|_| "dj.ibai".to_string());
        let mut stream = provider
            .connect(&room, ConnectionConfig::default())
            .await
            .expect("connect");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut received = 0;
        while let Ok(Some(item)) = tokio::time::timeout_at(deadline, stream.items.recv()).await {
            println!("{item:?}");
            received += 1;
            if received >= 10 {
                break;
            }
        }
        provider.disconnect(&mut stream.connection).await.unwrap();
        assert!(received > 0, "expected at least one danmu item");
    }
}
