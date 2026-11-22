//! Huya (虎牙) danmu provider.
//!
//! Implements danmu collection for the Huya streaming platform using the generic
//! WebSocket provider with TARS protocol for message encoding/decoding.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use reqwest::Client;
use rustc_hash::FxHashMap;
use tars_codec::{
    decode_response_zero_copy, decode_tars_struct, encode_tars_value, next_request_id,
    types::{TarsMessage, TarsRequestHeader, TarsValue},
};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::{
    DanmuProtocol, DanmuProtocolFactory, DanmuProtocolOutput, WebSocketDanmuProvider,
};
use crate::danmaku::{DanmuItem, DanmuMessage};
use crate::extractor::default::{DEFAULT_UA, default_client};
use crate::extractor::platforms::huya::huya_uri;
use crate::extractor::platforms::huya::web::PROFILE_INFO_REGEX;
use crate::extractor::platforms::huya::{
    GetLivingInfoRsp, HuyaSourceType, HuyaUserId, HuyaWsCmd, LiveAppUAEx, LiveLaunchReq,
    LiveUserBase, MessageNotice, WebSocketCommand, WsPushMessage, WsRegisterGroupReq,
    build_get_living_info_by_room_id_request, build_get_living_info_request,
};

use super::URL_REGEX;
use crate::extractor::utils::capture_group_1_owned;

/// Huya WebSocket server URL
const HUYA_WS_URL: &str = "wss://cdnws.api.huya.com:443";
const HUYA_BASE_URL: &str = "https://www.huya.com";

/// Heartbeat interval in seconds
const HEARTBEAT_INTERVAL_SECS: u64 = 60;

// Heartbeat message
const HEARTBEAT: &[u8] = &[
    0x00, 0x03, 0x1d, 0x00, 0x00, 0x69, 0x00, 0x00, 0x00, 0x69, 0x10, 0x03, 0x2c, 0x3c, 0x4c, 0x56,
    0x08, 0x6f, 0x6e, 0x6c, 0x69, 0x6e, 0x65, 0x75, 0x69, 0x66, 0x0f, 0x4f, 0x6e, 0x55, 0x73, 0x65,
    0x72, 0x48, 0x65, 0x61, 0x72, 0x74, 0x42, 0x65, 0x61, 0x74, 0x7d, 0x00, 0x00, 0x3c, 0x08, 0x00,
    0x01, 0x06, 0x04, 0x74, 0x52, 0x65, 0x71, 0x1d, 0x00, 0x00, 0x2f, 0x0a, 0x0a, 0x0c, 0x16, 0x00,
    0x26, 0x00, 0x36, 0x07, 0x61, 0x64, 0x72, 0x5f, 0x77, 0x61, 0x70, 0x46, 0x00, 0x0b, 0x12, 0x03,
    0xae, 0xf0, 0x0f, 0x22, 0x03, 0xae, 0xf0, 0x0f, 0x3c, 0x42, 0x6d, 0x52, 0x02, 0x60, 0x5c, 0x60,
    0x01, 0x7c, 0x82, 0x00, 0x0b, 0xb0, 0x1f, 0x9c, 0xac, 0x0b, 0x8c, 0x98, 0x0c, 0xa8, 0x0c, 0x20,
];

/// Huya Protocol Implementation
#[derive(Clone)]
pub struct HuyaDanmuProtocol {
    client: Client,
    base_url: std::borrow::Cow<'static, str>,
    /// Optional cookies for authenticated sessions
    cookies: Option<String>,
    /// Presenter UID resolved from extras, room HTML page, or `getLivingInfo`
    presenter_uid: Option<i64>,
    /// Whether `RegisterGroupReq` has been sent for the current connection
    joined_group: bool,
}

impl Default for HuyaDanmuProtocol {
    fn default() -> Self {
        Self {
            client: default_client(),
            base_url: std::borrow::Cow::Borrowed(HUYA_BASE_URL),
            cookies: None,
            presenter_uid: None,
            joined_group: false,
        }
    }
}

impl HuyaDanmuProtocol {
    /// Create a new HuyaDanmuProtocol instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new HuyaDanmuProtocol with cookies.
    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            cookies: Some(cookies.into()),
            ..Self::default()
        }
    }

    pub(crate) fn extract_presenter_uid_from_html(html: &str) -> Result<i64> {
        #[derive(serde::Deserialize)]
        struct ProfileUid {
            #[serde(default, deserialize_with = "super::models::deserialize_lp")]
            lp: i64,
        }

        let profile_info_str = PROFILE_INFO_REGEX
            .captures(html)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                DanmakuError::protocol("Could not find TT_PROFILE_INFO on Huya room page")
            })?;
        let profile_info: ProfileUid = serde_json::from_str(profile_info_str)
            .map_err(|e| DanmakuError::protocol(format!("Invalid Huya TT_PROFILE_INFO: {e}")))?;
        if profile_info.lp <= 0 {
            return Err(DanmakuError::connection(
                "Huya room presenter_uid not found",
            ));
        }
        Ok(profile_info.lp)
    }

    async fn resolve_vanity_presenter_uid(&self, slug: &str) -> Result<i64> {
        let url = format!("{}/{slug}", self.base_url.trim_end_matches('/'));
        let mut request = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::ORIGIN, HUYA_BASE_URL)
            .header(reqwest::header::REFERER, HUYA_BASE_URL);
        if let Some(cookies) = self
            .cookies
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            && let Ok(mut cookie_val) = reqwest::header::HeaderValue::from_str(cookies)
        {
            cookie_val.set_sensitive(true);
            request = request.header(reqwest::header::COOKIE, cookie_val);
        }
        let response = request.send().await.map_err(|e| {
            DanmakuError::connection(format!("Failed to fetch Huya room page: {e}"))
        })?;
        if !response.status().is_success() {
            return Err(DanmakuError::connection(format!(
                "Huya room page returned HTTP {}",
                response.status()
            )));
        }
        let html = response
            .text()
            .await
            .map_err(|e| DanmakuError::connection(format!("Failed to read Huya room page: {e}")))?;
        Self::extract_presenter_uid_from_html(&html)
    }
}

impl DanmuProtocolFactory for HuyaDanmuProtocol {
    type Protocol = Self;

    fn platform(&self) -> &str {
        "huya"
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
            base_url: self.base_url.clone(),
            cookies: self.cookies.clone(),
            presenter_uid: None,
            joined_group: false,
        }
    }
}

impl DanmuProtocol for HuyaDanmuProtocol {
    fn use_http_client(&mut self, client: Client) {
        self.client = client;
    }

    fn configure_connection(
        &mut self,
        cookies: Option<&str>,
        extras: Option<&HashMap<String, String>>,
    ) {
        if let Some(cookies) = cookies.map(str::trim).filter(|c| !c.is_empty()) {
            self.cookies = Some(cookies.to_owned());
        }
        self.presenter_uid = extras
            .and_then(|map| map.get("presenter_uid"))
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|&uid| uid > 0);
        self.joined_group = false;
    }

    async fn websocket_url(&mut self, room_id: &str) -> Result<String> {
        self.joined_group = false;
        let trimmed = room_id.trim();
        let is_numeric = !trimmed.is_empty()
            && trimmed.bytes().all(|b| b.is_ascii_digit())
            && trimmed.parse::<i64>().is_ok_and(|id| id > 0);
        if self.presenter_uid.is_none() && !is_numeric {
            let uid = self.resolve_vanity_presenter_uid(trimmed).await?;
            self.presenter_uid = Some(uid);
        }
        Ok(HUYA_WS_URL.to_string())
    }

    fn cookies(&self) -> Option<String> {
        self.cookies.clone()
    }

    async fn handshake_messages(&mut self, room_id: &str) -> Result<Vec<Message>> {
        let ua = format!(
            "webh5&{}&websocket",
            jiff::Timestamp::now().strftime("%y%m%d%H%M")
        );
        let device = "chrome";

        // 1. Living info packet: use presenter_uid when known, else query by numeric room_id
        let living_info_packet = if let Some(presenter_uid) = self.presenter_uid {
            Self::create_living_info_packet(presenter_uid, &ua, device)?
        } else {
            let trimmed = room_id.trim();
            let room_id_num: i64 = (!trimmed.is_empty()
                && trimmed.bytes().all(|b| b.is_ascii_digit()))
            .then(|| trimmed.parse::<i64>().ok())
            .flatten()
            .filter(|&id| id > 0)
            .ok_or_else(|| DanmakuError::connection("Invalid room ID".to_string()))?;
            Self::create_living_info_by_room_id_packet(room_id_num, &ua, device)?
        };

        // 2. Register packet
        let reg_packet = Self::create_do_launch_packet(&ua, device)?;

        Ok(vec![
            Message::Binary(living_info_packet),
            Message::Binary(reg_packet),
        ])
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
        room_id: &str,
    ) -> Result<DanmuProtocolOutput> {
        match message {
            Message::Binary(data) => self.parse_socket_command(data, room_id),
            _ => Ok(DanmuProtocolOutput::default()),
        }
    }
}

// Re-export the provider type
pub type HuyaDanmuProvider = WebSocketDanmuProvider<HuyaDanmuProtocol>;

pub fn create_huya_danmu_provider() -> HuyaDanmuProvider {
    HuyaDanmuProvider::with_factory(HuyaDanmuProtocol::default(), None)
}

// Static helper methods for TARS encoding/decoding
impl HuyaDanmuProtocol {
    pub fn create_living_info_packet(presenter_uid: i64, ua: &str, device: &str) -> Result<Bytes> {
        let packet = build_get_living_info_request(presenter_uid, ua, device)
            .map_err(|e| DanmakuError::connection(format!("Failed to build packet: {}", e)))?;
        Self::wrap_register_req(packet)
    }

    pub fn create_living_info_by_room_id_packet(
        room_id: i64,
        ua: &str,
        device: &str,
    ) -> Result<Bytes> {
        let packet = build_get_living_info_by_room_id_request(room_id, ua, device)
            .map_err(|e| DanmakuError::connection(format!("Failed to build packet: {}", e)))?;
        Self::wrap_register_req(packet)
    }

    fn wrap_register_req(packet: Bytes) -> Result<Bytes> {
        let req = WebSocketCommand::new(
            HuyaWsCmd::RegisterReq as i32,
            packet,
            0,
            String::new(),
            0,
            0,
            String::new(),
        );
        encode_tars_value(&TarsValue::from(req))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode packet: {}", e)))
            .map(bytes::BytesMut::freeze)
    }

    pub fn create_do_launch_packet(ua: &str, device: &str) -> Result<Bytes> {
        let user_id = HuyaUserId::new(0, String::new(), String::new(), ua.to_string())
            .with_device_info(device.to_string());
        let user_base = LiveUserBase::new(HuyaSourceType::PcWeb as i32, 0, LiveAppUAEx::default());
        let live_launch_req = LiveLaunchReq::new(user_id, user_base, true);

        let body_bytes = encode_tars_value(&TarsValue::from(live_launch_req))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode body: {}", e)))?
            .freeze();

        let mut body_map = FxHashMap::default();
        body_map.insert("tReq".to_string(), body_bytes);

        let message = TarsMessage {
            header: TarsRequestHeader {
                version: 3,
                packet_type: 0,
                message_type: 0,
                request_id: next_request_id(),
                servant_name: "liveui".to_string(),
                func_name: "doLaunch".to_string(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body: body_map,
        };

        let encoded = tars_codec::encode_request(&message)
            .map_err(|e| {
                DanmakuError::connection(format!("Failed to encode room join packet: {}", e))
            })?
            .freeze();

        Self::wrap_register_req(encoded)
    }

    pub fn create_create_join_group_packet(presenter_uid: i64) -> Result<Bytes> {
        let register_req = WsRegisterGroupReq::new(
            vec![
                format!("live:{}", presenter_uid),
                format!("chat:{}", presenter_uid),
            ],
            String::new(), // token is empty
        );
        let register_bytes = encode_tars_value(&TarsValue::from(register_req))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode register: {}", e)))?
            .freeze();
        let socket_cmd = WebSocketCommand::new(
            HuyaWsCmd::RegisterGroupReq as i32,
            register_bytes,
            0,
            String::new(),
            0,
            0,
            String::new(),
        );
        Ok(encode_tars_value(&TarsValue::from(socket_cmd))
            .map_err(|e| DanmakuError::connection(format!("Failed to encode socket cmd: {}", e)))?
            .freeze())
    }

    fn maybe_join_group_outbound(&mut self) -> Vec<Message> {
        if self.joined_group {
            return Vec::new();
        }
        let Some(presenter_uid) = self.presenter_uid.filter(|&uid| uid > 0) else {
            return Vec::new();
        };
        match Self::create_create_join_group_packet(presenter_uid) {
            Ok(packet) => {
                debug!(
                    presenter_uid,
                    "Queued join group packet for live/chat groups"
                );
                self.joined_group = true;
                vec![Message::Binary(packet)]
            }
            Err(e) => {
                debug!("Failed to create join group packet: {}", e);
                Vec::new()
            }
        }
    }

    /// Parse incoming WebSocket command and handle protocol-level responses
    fn parse_socket_command(
        &mut self,
        data: &Bytes,
        _room_id: &str,
    ) -> Result<DanmuProtocolOutput> {
        if data.len() < 4 {
            return Ok(DanmuProtocolOutput::default());
        }

        // Decode the WebSocketCommand wrapper zero-copy via Bytes refcount clone
        let tars_val = match decode_tars_struct(data.clone()) {
            Ok(v) => v,
            Err(e) => {
                debug!("Failed to decode TARS value from socket command: {}", e);
                return Ok(DanmuProtocolOutput::default());
            }
        };

        let socket_cmd = match WebSocketCommand::try_from(tars_val) {
            Ok(cmd) => cmd,
            Err(e) => {
                debug!("Failed to convert TARS value to WebSocketCommand: {}", e);
                return Ok(DanmuProtocolOutput::default());
            }
        };

        let cmd_type = socket_cmd.cmd_type();
        match cmd_type {
            // HeartbeatRsp (2) - just acknowledge
            2 => {
                debug!("Received heartbeat response");
                Ok(DanmuProtocolOutput::default())
            }
            // RegisterRsp (4) - handle getLivingInfo (presenter_uid discovery) and doLaunch
            4 => {
                let inner_data = socket_cmd.data().clone();
                if inner_data.is_empty() {
                    debug!("Received empty RegisterRsp");
                    return Ok(DanmuProtocolOutput::default());
                }

                let message = match decode_response_zero_copy(inner_data) {
                    Ok(msg) => msg,
                    Err(e) => {
                        debug!("Failed to decode RegisterRsp inner message: {}", e);
                        return Ok(DanmuProtocolOutput::default());
                    }
                };

                match message.header.func_name.as_str() {
                    "getLivingInfo" => {
                        if let Some(resp_bytes) = message.body.get("tRsp")
                            && let Ok(val) = tars_codec::de::from_bytes(resp_bytes.clone())
                            && let Ok(rsp) = GetLivingInfoRsp::try_from(val)
                        {
                            let uid = if rsp.t_notice.l_presenter_uid > 0 {
                                rsp.t_notice.l_presenter_uid
                            } else {
                                rsp.t_stream_setting_notice.l_presenter_uid
                            };
                            if uid > 0 && self.presenter_uid.is_none() {
                                debug!(
                                    presenter_uid = uid,
                                    "Resolved presenter_uid from getLivingInfo"
                                );
                                self.presenter_uid = Some(uid);
                            }
                        }
                        let outbound = self.maybe_join_group_outbound();
                        Ok(DanmuProtocolOutput::outbound(outbound))
                    }
                    "doLaunch" => {
                        debug!("Received doLaunch response");
                        let outbound = self.maybe_join_group_outbound();
                        Ok(DanmuProtocolOutput::outbound(outbound))
                    }
                    other => {
                        debug!("Received RegisterRsp with func_name: {}", other);
                        Ok(DanmuProtocolOutput::default())
                    }
                }
            }
            // WupRsp (7) - Push messages in WsPushMessage format
            7 => {
                let inner_data = socket_cmd.data().clone();
                if inner_data.is_empty() {
                    return Ok(DanmuProtocolOutput::default());
                }

                let push_msg_val = match decode_tars_struct(inner_data) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("Failed to decode WsPushMessage TARS: {}", e);
                        return Ok(DanmuProtocolOutput::default());
                    }
                };

                match WsPushMessage::try_from(push_msg_val) {
                    Ok(push_msg) => {
                        // we only want 1400 uri (danmu messages)
                        if push_msg.i_uri != huya_uri::MESSAGE_NOTICE as i64 {
                            return Ok(DanmuProtocolOutput::default());
                        }

                        // Parse the inner s_msg data as MessageNotice
                        if !push_msg.s_msg.is_empty() {
                            let notice_val = match decode_tars_struct(push_msg.s_msg) {
                                Ok(v) => v,
                                Err(e) => {
                                    debug!("Failed to decode MessageNotice TARS: {}", e);
                                    return Ok(DanmuProtocolOutput::default());
                                }
                            };

                            match MessageNotice::try_from(notice_val) {
                                Ok(danmu_content) => {
                                    let username = &danmu_content.t_user_info.s_nick_name;
                                    let content = &danmu_content.s_content;
                                    let color = danmu_content.t_bullet_format.i_color;

                                    if !content.is_empty() {
                                        let mut danmu = DanmuMessage::chat(
                                            push_msg.l_msg_id.to_string(),
                                            danmu_content.t_user_info.l_uid.to_string(),
                                            username.clone(),
                                            content.clone(),
                                        );
                                        if let Some(color) = bullet_color(color) {
                                            danmu = danmu.with_color(color);
                                        }
                                        return Ok(vec![DanmuItem::Message(danmu)].into());
                                    }
                                }
                                Err(e) => {
                                    debug!("Failed to convert to MessageNotice: {}", e);
                                }
                            }
                        }
                        Ok(DanmuProtocolOutput::default())
                    }
                    Err(e) => {
                        debug!("Failed to convert to WsPushMessage: {}", e);
                        Ok(DanmuProtocolOutput::default())
                    }
                }
            }
            // RegisterGroupRsp (17) - acknowledgment of group registration
            17 => {
                debug!("Successfully registered to group");
                Ok(DanmuProtocolOutput::default())
            }
            // Other types
            _ => Ok(DanmuProtocolOutput::default()),
        }
    }
}

/// The `#RRGGBB` colour of a chat message, or `None` for an uncoloured one.
///
/// Huya marks ordinary chat with `255`, which read as RGB would turn every
/// plain message blue. `-1` and `0` also mean uncoloured, and anything outside
/// 24-bit RGB is not a colour.
fn bullet_color(i_color: i32) -> Option<String> {
    match i_color {
        255 => None,
        1..=0xFF_FFFF => Some(format!("#{i_color:06X}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::danmaku::{ConnectionConfig, provider::DanmuProvider};

    /// Test that living info packet can be created
    #[test]
    fn test_create_living_info_packet() {
        let result = HuyaDanmuProtocol::create_living_info_packet(
            294636272,
            "webh5&2512200618&websocket",
            "chrome",
        );
        assert!(result.is_ok());
        let packet = result.unwrap();
        assert!(!packet.is_empty());
    }

    /// Test that do launch packet can be created
    #[test]
    fn test_create_do_launch_packet() {
        let result =
            HuyaDanmuProtocol::create_do_launch_packet("webh5&2512200618&websocket", "chrome");
        assert!(result.is_ok());
        let packet = result.unwrap();
        assert!(!packet.is_empty());
    }

    /// Test that join group packet can be created
    #[test]
    fn test_create_join_group_packet() {
        let result = HuyaDanmuProtocol::create_create_join_group_packet(294636272);
        assert!(result.is_ok());
        let packet = result.unwrap();
        assert!(!packet.is_empty());
    }

    fn encode_register_rsp(func_name: &str, body: FxHashMap<String, Bytes>) -> Bytes {
        let msg = TarsMessage {
            header: TarsRequestHeader {
                version: 3,
                packet_type: 0,
                message_type: 0,
                request_id: 1,
                servant_name: "liveui".to_string(),
                func_name: func_name.to_string(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body,
        };
        let inner = tars_codec::encode_request(&msg).unwrap().freeze();
        let cmd = WebSocketCommand::new(
            HuyaWsCmd::RegisterRsp as i32,
            inner,
            0,
            String::new(),
            0,
            0,
            String::new(),
        );
        encode_tars_value(&TarsValue::from(cmd)).unwrap().freeze()
    }

    fn decode_group_ids_from_outbound(msg: &Message) -> Vec<String> {
        let Message::Binary(bytes) = msg else {
            panic!("expected binary RegisterGroupReq");
        };
        let cmd_val = decode_tars_struct(bytes.clone()).unwrap();
        let cmd = WebSocketCommand::try_from(cmd_val).unwrap();
        assert_eq!(cmd.cmd_type(), HuyaWsCmd::RegisterGroupReq as i32);
        let reg_val = decode_tars_struct(cmd.data().clone()).unwrap();
        let reg = WsRegisterGroupReq::try_from(reg_val).unwrap();
        let TarsValue::Struct(mut fields) = TarsValue::from(reg) else {
            unreachable!();
        };
        fields
            .remove(&0)
            .unwrap()
            .try_into_list()
            .unwrap()
            .into_iter()
            .map(|v| v.try_into_string().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn configured_presenter_uid_and_get_living_info_discovery_join_correct_groups() {
        use crate::extractor::platforms::huya::tars::StreamSettingNotice;

        // 1. Pre-supplied presenter_uid via configure_connection joins live:{uid} and chat:{uid}.
        let mut proto = HuyaDanmuProtocol::default();
        let extras = HashMap::from([("presenter_uid".to_string(), "1199511627776".to_string())]);
        proto.configure_connection(Some("yyuid=7"), Some(&extras));

        let launch_rsp = encode_register_rsp("doLaunch", FxHashMap::default());
        let (_, outbound) = proto
            .decode_message(&Message::Binary(launch_rsp.clone()), "660000")
            .await
            .unwrap()
            .into_parts();
        assert_eq!(outbound.len(), 1);
        assert_eq!(
            decode_group_ids_from_outbound(&outbound[0]),
            vec!["live:1199511627776", "chat:1199511627776"]
        );
        // Subsequent RegisterRsp must not re-send RegisterGroupReq.
        let (_, dup_outbound) = proto
            .decode_message(&Message::Binary(launch_rsp), "660000")
            .await
            .unwrap()
            .into_parts();
        assert!(dup_outbound.is_empty());

        // 2. Numeric room_id without presenter_uid queries getLivingInfo by room_id and extracts l_presenter_uid.
        let mut proto_num = HuyaDanmuProtocol::default();
        let hs = proto_num.handshake_messages("660000").await.unwrap();
        assert_eq!(hs.len(), 2);

        let rsp_struct = GetLivingInfoRsp {
            b_is_living: 1,
            t_stream_setting_notice: StreamSettingNotice {
                l_presenter_uid: 99887766,
                ..Default::default()
            },
            ..Default::default()
        };
        let trsp_bytes = tars_codec::encode_tars_value_wrapped(&TarsValue::from(rsp_struct))
            .unwrap()
            .freeze();
        let mut body = FxHashMap::default();
        body.insert("tRsp".to_string(), trsp_bytes);
        let living_rsp = encode_register_rsp("getLivingInfo", body);

        let (_, outbound_num) = proto_num
            .decode_message(&Message::Binary(living_rsp), "660000")
            .await
            .unwrap()
            .into_parts();
        assert_eq!(outbound_num.len(), 1);
        assert_eq!(
            decode_group_ids_from_outbound(&outbound_num[0]),
            vec!["live:99887766", "chat:99887766"]
        );
    }

    #[tokio::test]
    async fn vanity_slug_resolves_presenter_uid_from_room_page() {
        use crate::extractor::test_support::serve_once;

        let page_url = serve_once(
            "200 OK",
            r#"<html><script>var TT_PROFILE_INFO = {"lp":"44556677","aid":1};</script></html>"#,
        )
        .await;
        let base_origin = page_url.trim_end_matches('/').to_string();

        let mut proto = HuyaDanmuProtocol {
            base_url: std::borrow::Cow::Owned(base_origin),
            ..Default::default()
        };
        proto.websocket_url("baozou").await.unwrap();
        assert_eq!(proto.presenter_uid, Some(44556677));
    }

    /// Integration test: Connect to a real Huya room and verify handshake
    /// This test requires network access and a live Huya room
    #[tokio::test]
    #[ignore] // Run with: cargo test --package platforms -- --ignored test_real_connection
    async fn test_real_connection() {
        use std::time::Duration;
        use tokio::time::timeout;

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();

        // Use a popular room that's likely to be live
        let room_id = "660000";

        let provider = create_huya_danmu_provider();

        // Test connection with timeout
        let connect_result = timeout(
            Duration::from_secs(10),
            provider.connect(room_id, ConnectionConfig::default()),
        )
        .await;

        assert!(connect_result.is_ok(), "Connection timed out");
        let stream = connect_result.unwrap().expect("Failed to connect");
        let mut connection = stream.connection;
        let mut items = stream.items;

        assert!(connection.is_connected, "Connection should be connected");

        // Try to receive messages for a few seconds
        let mut message_count = 0;
        let start = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(600) {
            match timeout(Duration::from_millis(500), items.recv()).await {
                Ok(Some(item)) => match item {
                    crate::danmaku::DanmuItem::Message(msg) => {
                        println!("{}: {}", msg.username, msg.content);
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
                    // Timeout, continue
                }
            }
        }

        println!("Received {} danmu messages", message_count);

        // Disconnect
        let disconnect_result = provider.disconnect(&mut connection).await;
        assert!(disconnect_result.is_ok(), "Failed to disconnect");
    }

    /// Integration test: Verify WebSocket command decoding
    #[tokio::test]
    #[ignore]
    async fn test_websocket_command_flow() {
        use futures::{SinkExt, StreamExt};
        use std::time::Duration;
        use tokio::time::timeout;
        use tokio_tungstenite::connect_async;

        let room_id = "660000";
        let mut protocol = HuyaDanmuProtocol::default();

        // Connect directly to verify the flow
        let ws_url = protocol.websocket_url(room_id).await.unwrap();
        println!("Connecting to: {}", ws_url);

        let (mut ws_stream, _) = timeout(Duration::from_secs(5), connect_async(&ws_url))
            .await
            .expect("Connection timed out")
            .expect("Failed to connect");

        // Send handshake messages
        let handshake_msgs = protocol.handshake_messages(room_id).await.unwrap();
        for msg in handshake_msgs {
            ws_stream.send(msg).await.expect("Failed to send handshake");
        }
        println!("Sent handshake messages");

        // Receive a few messages and verify WebSocketCommand decoding
        let mut response_count = 0;
        let start = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(10) && response_count < 20 {
            tokio::select! {
                msg_result = ws_stream.next() => {
                    match msg_result {
                        Some(Ok(Message::Binary(data))) => {
                            // Try to decode as WebSocketCommand using manual pattern
                            if let Ok(tars_val) = tars_codec::decode_tars_value(data.clone()) && let Ok(cmd) = WebSocketCommand::try_from(tars_val) {
                                    let _cmd_type = cmd.cmd_type();
                                    response_count += 1;

                                    // Parse through our protocol
                                    match protocol.parse_socket_command(&data, room_id) {
                                        Ok(output) => {
                                            let (items, outbound) = output.into_parts();
                                            for response in outbound {
                                                ws_stream.send(response).await.expect("Failed to send response");
                                            }
                                            for item in items {
                                                match item {
                                                    crate::danmaku::DanmuItem::Message(danmu) => {
                                                        println!(
                                                            "  -> Danmu: {} - {}",
                                                            danmu.username, danmu.content
                                                        );
                                                    }
                                                    crate::danmaku::DanmuItem::Control(control) => {
                                                        println!("  -> Control: {:?}", control);
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            println!("  -> Parse error: {}", e);
                                        }
                                    }
                                }
                        }
                        Some(Ok(other)) => {
                            println!("Received other message: {:?}", other);
                        }
                        Some(Err(e)) => {
                            println!("WebSocket error: {}", e);
                            break;
                        }
                        None => {
                            println!("WebSocket closed");
                            break;
                        }
                    }
                }
            }
        }

        println!("Received {} WebSocket commands", response_count);
        assert!(
            response_count > 0,
            "Should have received at least one WebSocket command"
        );

        let _ = ws_stream.close(None).await;
    }
}
