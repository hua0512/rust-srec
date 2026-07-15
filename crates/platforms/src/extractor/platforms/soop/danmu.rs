//! SOOP guest chat WebSocket protocol.
//!
//! Open rooms: SVC_LOGIN guest flag 16 → SVC_JOINCH with CHATNO + FTK →
//! SVC_CHATMESG / gift SVCs. Binary frames with ESC+TAB header and FF-separated
//! fields (see research notes in `soop` investigation).

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use chrono::Utc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::http::{HeaderMap, HeaderValue, header};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};

use crate::danmaku::error::{DanmakuError, Result};
use crate::danmaku::websocket::ws_headers_origin_ua;
use crate::danmaku::websocket::{DanmuProtocol, WebSocketDanmuProvider};
use crate::danmaku::{DanmuItem, DanmuMessage};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::platforms::soop::URL_REGEX;
use crate::extractor::utils::capture_group_1_owned;

const FF: char = '\u{000c}';
const DC1: char = '\u{0011}';
const DC2: char = '\u{0012}';
const ACK: char = '\u{0006}';

const SVC_KEEPALIVE: i32 = 0;
const SVC_LOGIN: i32 = 1;
const SVC_JOINCH: i32 = 2;
const SVC_CHATMESG: i32 = 5;
const SVC_SENDBALLOON: i32 = 18;
const SVC_SENDFANLETTER: i32 = 20;
const SVC_SENDBALLOONSUB: i32 = 33;
const SVC_SENDFANLETTERSUB: i32 = 34;
const SVC_CHOCOLATE: i32 = 37;
const SVC_CHOCOLATESUB: i32 = 38;
const SVC_SUPERCHAT: i32 = 41;
const SVC_SENDSUBSCRIPTION: i32 = 108;
const SVC_GEM_ITEM_SEND: i32 = 120;

const HEARTBEAT_INTERVAL_SECS: u64 = 60;
const ORIGIN: &str = "https://play.sooplive.com";

#[derive(Clone, Default)]
struct ChatSession {
    chatno: String,
    ftk: String,
    chdomain: String,
    chpt: i64,
    bjid: String,
    room_password: String,
}

#[derive(Clone, Default)]
pub struct SoopDanmuProtocol {
    session: ChatSession,
    cookies: Option<String>,
}

impl SoopDanmuProtocol {
    pub fn new() -> Self {
        Self::default()
    }

    fn make_packet(svc: i32, fields: &[&str]) -> Bytes {
        let mut body = String::new();
        for field in fields {
            body.push(FF);
            body.push_str(field);
        }
        body.push(FF);
        let body_b = body.into_bytes();
        let header = format!("\x1b\x09{svc:04}{:06}00", body_b.len());
        let mut out = header.into_bytes();
        out.extend_from_slice(&body_b);
        Bytes::from(out)
    }

    fn parse_packets(data: &[u8]) -> Vec<(i32, Vec<String>)> {
        let mut out = Vec::new();
        let mut i = 0;
        let n = data.len();
        while i < n {
            if i + 2 > n || data[i] != 0x1b || data[i + 1] != 0x09 {
                if let Some(rel) = data[i..].windows(2).position(|w| w == [0x1b, 0x09]) {
                    i += rel;
                } else {
                    break;
                }
            }
            if i + 14 > n {
                break;
            }
            let svc = match std::str::from_utf8(&data[i + 2..i + 6])
                .ok()
                .and_then(|s| s.parse::<i32>().ok())
            {
                Some(v) => v,
                None => {
                    i += 1;
                    continue;
                }
            };
            let length = match std::str::from_utf8(&data[i + 6..i + 12])
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
            {
                Some(v) => v,
                None => {
                    i += 1;
                    continue;
                }
            };
            let end = i + 14 + length;
            if end > n {
                break;
            }
            let body = &data[i + 14..end];
            let text = String::from_utf8_lossy(body);
            let mut parts: Vec<String> = text.split(FF).map(str::to_string).collect();
            if parts.first().is_some_and(|s| s.is_empty()) {
                parts.remove(0);
            }
            if parts.last().is_some_and(|s| s.is_empty()) {
                parts.pop();
            }
            out.push((svc, parts));
            i = end;
        }
        out
    }

    fn join_payload(session: &ChatSession) -> String {
        let log = format!(
            "{ACK}&{ACK}set_bps{ACK}={ACK}4000\
             {ACK}&{ACK}view_bps{ACK}={ACK}4000\
             {ACK}&{ACK}quality{ACK}={ACK}ori\
             {ACK}&{ACK}uuid{ACK}={ACK}rust-srec\
             {ACK}&{ACK}lowlatency{ACK}={ACK}1\
             {ACK}&{ACK}mode{ACK}={ACK}landing"
        );
        format!(
            "log{DC1}{log}{DC2}pwd{DC1}{}{DC2}auth_info{DC1}NULL{DC2}pver{DC1}2{DC2}access_system{DC1}html5{DC2}",
            session.room_password
        )
    }

    fn gift_name(svc: i32) -> Option<&'static str> {
        match svc {
            SVC_SENDBALLOON | SVC_SENDBALLOONSUB => Some("balloon"),
            SVC_SENDFANLETTER | SVC_SENDFANLETTERSUB => Some("fan_letter"),
            SVC_CHOCOLATE | SVC_CHOCOLATESUB => Some("chocolate"),
            SVC_SUPERCHAT => Some("superchat"),
            SVC_SENDSUBSCRIPTION => Some("subscription"),
            SVC_GEM_ITEM_SEND => Some("gem"),
            _ => None,
        }
    }

    fn parse_chat(fields: &[String]) -> Option<DanmuMessage> {
        let content = fields.first()?.as_str();
        if content.is_empty() {
            return None;
        }
        let uid = fields.get(1).map(String::as_str).unwrap_or("");
        let nickname = fields
            .get(5)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(uid);
        let id = format!("soop-{}-{}", uid, Utc::now().timestamp_millis());
        Some(
            DanmuMessage::chat(&id, uid, nickname, content)
                .with_timestamp(Utc::now())
                .with_metadata("platform", serde_json::json!("soop")),
        )
    }

    fn parse_gift(svc: i32, fields: &[String]) -> Option<DanmuMessage> {
        let gift_name = Self::gift_name(svc)?;
        // Field layouts differ by SVC (see research client). Balloon/chocolate
        // share [bj, uid, nick, count, …]; fan-letter style uses shifted indices.
        let (uid, nickname, count) = match svc {
            SVC_SENDFANLETTER | SVC_SENDFANLETTERSUB => {
                let uid = fields.get(2).map(String::as_str).unwrap_or("");
                let nick = fields
                    .get(3)
                    .map(String::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(uid);
                let count = fields
                    .get(7)
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1)
                    .max(1);
                (uid, nick, count)
            }
            _ => {
                let uid = fields.get(1).map(String::as_str).unwrap_or("");
                let nick = fields
                    .get(2)
                    .map(String::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(uid);
                let count = fields
                    .get(3)
                    .and_then(|s| s.parse::<u32>().ok())
                    .or_else(|| {
                        fields
                            .iter()
                            .skip(3)
                            .take(3)
                            .find_map(|s| s.parse::<u32>().ok())
                    })
                    .unwrap_or(1)
                    .max(1);
                (uid, nick, count)
            }
        };
        let id = format!("soop-gift-{svc}-{}", Utc::now().timestamp_millis());
        Some(
            DanmuMessage::gift(&id, uid, nickname, gift_name, count)
                .with_timestamp(Utc::now())
                .with_metadata("platform", serde_json::json!("soop"))
                .with_metadata("soop_svc", serde_json::json!(svc)),
        )
    }

    fn is_join_failure(fields: &[String]) -> bool {
        let Some(first) = fields.first() else {
            return true;
        };
        if first.chars().all(|c| c.is_ascii_digit()) && fields.len() >= 2 {
            return false;
        }
        let blob = fields.join(" ").to_lowercase();
        blob.contains("비밀번호")
            || blob.contains("password")
            || blob.contains("틀렸")
            || blob.contains("wrong")
            || blob.contains("실패")
            || blob.contains("fail")
            || blob.contains("denied")
            || blob.contains("error")
            || !first.chars().all(|c| c.is_ascii_digit())
    }
}

impl DanmuProtocol for SoopDanmuProtocol {
    fn platform(&self) -> &str {
        "soop"
    }

    fn supports_url(&self, url: &str) -> bool {
        URL_REGEX.is_match(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        capture_group_1_owned(&URL_REGEX, url)
    }

    async fn websocket_url(&self, room_id: &str) -> Result<String> {
        let session = self.session.clone();
        let bjid = if session.bjid.is_empty() {
            room_id.to_string()
        } else {
            session.bjid
        };
        if session.chdomain.is_empty() || session.chpt <= 0 {
            return Err(DanmakuError::connection(
                "SOOP chat host missing — extract must populate chdomain/chpt extras",
            ));
        }
        let port = session.chpt + 1;
        let bjid_path = bjid
            .split('(')
            .next()
            .unwrap_or(bjid.as_str())
            .trim()
            .to_string();
        Ok(format!(
            "wss://{}:{}/Websocket/{}",
            session.chdomain, port, bjid_path
        ))
    }

    fn headers(&self, _room_id: &str) -> HeaderMap {
        let mut headers = ws_headers_origin_ua(ORIGIN, DEFAULT_UA);
        // Guest chat requires the `chat` subprotocol.
        headers.insert(
            header::HeaderName::from_static("sec-websocket-protocol"),
            HeaderValue::from_static("chat"),
        );
        headers
    }

    fn send_cookie_header(&self) -> bool {
        // Cookies only needed for login-gated rooms; open rooms work without.
        self.cookies.is_some()
    }

    fn cookies(&self) -> Option<String> {
        self.cookies.clone()
    }

    fn configure_connection(
        &mut self,
        cookies: Option<&str>,
        extras: Option<&HashMap<String, String>>,
    ) {
        if let Some(cookies) = cookies.filter(|s| !s.is_empty()) {
            self.cookies = Some(cookies.to_string());
        }

        let Some(extras) = extras else {
            return;
        };

        let session = &mut self.session;
        if let Some(v) = extras.get("chatno").filter(|s| !s.is_empty()) {
            session.chatno = v.clone();
        }
        if let Some(v) = extras.get("ftk").filter(|s| !s.is_empty()) {
            session.ftk = v.clone();
        }
        if let Some(v) = extras.get("chdomain").filter(|s| !s.is_empty()) {
            session.chdomain = v.to_ascii_lowercase();
        }
        if let Some(v) = extras.get("chpt").and_then(|s| s.parse().ok()) {
            session.chpt = v;
        }
        if let Some(v) = extras
            .get("bjid")
            .or_else(|| extras.get("channel_id"))
            .filter(|s| !s.is_empty())
        {
            session.bjid = v.clone();
        }
        if let Some(v) = extras
            .get("stream_password")
            .or_else(|| extras.get("pwd"))
            .filter(|s| !s.is_empty())
        {
            session.room_password = v.clone();
        }
    }

    async fn handshake_messages(&self, _room_id: &str) -> Result<Vec<Message>> {
        // Guest login: empty ticket, empty password, flag 16.
        let login = Self::make_packet(SVC_LOGIN, &["", "", "16"]);
        Ok(vec![Message::Binary(login)])
    }

    fn heartbeat_message(&self) -> Option<Message> {
        Some(Message::Binary(Self::make_packet(SVC_KEEPALIVE, &[])))
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
        match message {
            Message::Binary(data) => {
                let packets = Self::parse_packets(data);
                let mut items = Vec::new();

                for (svc, fields) in packets {
                    match svc {
                        SVC_LOGIN => {
                            let session = self.session.clone();
                            // JOIN requires CHATNO from type=live extras — never the bj id
                            // (room_id). Missing FTK also fails join on open rooms.
                            if session.chatno.is_empty() {
                                return Err(DanmakuError::connection(
                                    "SOOP chat join missing chatno in connection extras",
                                ));
                            }
                            if session.ftk.is_empty() {
                                return Err(DanmakuError::connection(
                                    "SOOP chat join missing ftk in connection extras",
                                ));
                            }
                            let mode = Self::join_payload(&session);
                            let join = Self::make_packet(
                                SVC_JOINCH,
                                &[
                                    session.chatno.as_str(),
                                    session.ftk.as_str(),
                                    "0",
                                    "",
                                    mode.as_str(),
                                ],
                            );
                            if let Err(e) = tx.send(Message::Binary(join)).await {
                                warn!(error = %e, "failed to send SOOP JOINCH");
                            } else {
                                debug!(
                                    chatno = %session.chatno,
                                    room_id,
                                    "SOOP JOINCH sent after LOGIN"
                                );
                            }
                        }
                        SVC_JOINCH => {
                            if Self::is_join_failure(&fields) {
                                let err = fields.join(" ");
                                return Err(DanmakuError::connection(format!(
                                    "SOOP chat join failed: {err}"
                                )));
                            }
                            debug!(?fields, "SOOP chat joined");
                        }
                        SVC_CHATMESG => {
                            if let Some(msg) = Self::parse_chat(&fields) {
                                items.push(DanmuItem::Message(msg));
                            }
                        }
                        SVC_SENDBALLOON | SVC_SENDBALLOONSUB | SVC_SENDFANLETTER
                        | SVC_SENDFANLETTERSUB | SVC_CHOCOLATE | SVC_CHOCOLATESUB
                        | SVC_SUPERCHAT | SVC_SENDSUBSCRIPTION | SVC_GEM_ITEM_SEND => {
                            if let Some(msg) = Self::parse_gift(svc, &fields) {
                                items.push(DanmuItem::Message(msg));
                            }
                        }
                        SVC_KEEPALIVE => {}
                        _ => {}
                    }
                }

                Ok(items)
            }
            Message::Text(text) => {
                debug!(%text, "SOOP unexpected text frame");
                Ok(vec![])
            }
            Message::Close(frame) => Err(DanmakuError::connection(format!(
                "SOOP chat closed: {frame:?}"
            ))),
            _ => Ok(vec![]),
        }
    }
}

/// Create a SOOP guest danmu provider.
pub fn create_soop_danmu_provider() -> WebSocketDanmuProvider<SoopDanmuProtocol> {
    WebSocketDanmuProvider::with_protocol(SoopDanmuProtocol::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_roundtrip_shape() {
        let pkt = SoopDanmuProtocol::make_packet(SVC_LOGIN, &["", "", "16"]);
        assert_eq!(&pkt[..2], b"\x1b\x09");
        let parsed = SoopDanmuProtocol::parse_packets(&pkt);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, SVC_LOGIN);
        assert_eq!(parsed[0].1, vec!["", "", "16"]);
    }

    #[test]
    fn parse_concatenated_packets() {
        let a = SoopDanmuProtocol::make_packet(SVC_KEEPALIVE, &[]);
        let b = SoopDanmuProtocol::make_packet(SVC_CHATMESG, &["hi", "u1", "0", "0", "ko", "nick"]);
        let mut buf = a.to_vec();
        buf.extend_from_slice(&b);
        let parsed = SoopDanmuProtocol::parse_packets(&buf);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].1[0], "hi");
        assert_eq!(parsed[1].1[5], "nick");
    }

    #[test]
    fn join_failure_detection() {
        assert!(!SoopDanmuProtocol::is_join_failure(&[
            "7604".into(),
            "bj".into(),
            "0".into()
        ]));
        assert!(SoopDanmuProtocol::is_join_failure(&[
            "비밀번호가 틀렸습니다.".into()
        ]));
    }

    #[test]
    fn configure_reads_extras() {
        let mut p = SoopDanmuProtocol::new();
        let mut extras = HashMap::new();
        extras.insert("chatno".into(), "7604".into());
        extras.insert("ftk".into(), "ticket".into());
        extras.insert("chdomain".into(), "chat-AA.sooplive.com".into());
        extras.insert("chpt".into(), "8040".into());
        extras.insert("bjid".into(), "example".into());
        extras.insert("stream_password".into(), "secret".into());
        p.configure_connection(None, Some(&extras));
        let s = p.session.clone();
        assert_eq!(s.chatno, "7604");
        assert_eq!(s.ftk, "ticket");
        assert_eq!(s.chdomain, "chat-aa.sooplive.com");
        assert_eq!(s.chpt, 8040);
        assert_eq!(s.bjid, "example");
        assert_eq!(s.room_password, "secret");
    }

    #[test]
    fn cloned_protocols_keep_connection_state_isolated() {
        let base = SoopDanmuProtocol::new();
        let mut first = base.clone();
        let mut second = base.clone();

        let mut first_extras = HashMap::new();
        first_extras.insert("chatno".into(), "1001".into());
        first_extras.insert("ftk".into(), "first-ticket".into());
        first.configure_connection(None, Some(&first_extras));

        let mut second_extras = HashMap::new();
        second_extras.insert("chatno".into(), "2002".into());
        second_extras.insert("ftk".into(), "second-ticket".into());
        second.configure_connection(None, Some(&second_extras));

        assert_eq!(first.session.chatno, "1001");
        assert_eq!(first.session.ftk, "first-ticket");
        assert_eq!(second.session.chatno, "2002");
        assert_eq!(second.session.ftk, "second-ticket");
    }

    #[tokio::test]
    async fn login_without_chatno_errors() {
        let p = SoopDanmuProtocol::new();
        let login = SoopDanmuProtocol::make_packet(SVC_LOGIN, &["", "", "16"]);
        let (tx, _rx) = mpsc::channel(4);
        let err = p
            .decode_message(&Message::Binary(login), "bjid", &tx)
            .await
            .expect_err("must require chatno");
        assert!(err.to_string().contains("chatno"));
    }

    #[tokio::test]
    async fn login_sends_join_with_chatno_not_bjid() {
        let mut p = SoopDanmuProtocol::new();
        let mut extras = HashMap::new();
        extras.insert("chatno".into(), "7604".into());
        extras.insert("ftk".into(), "ticket".into());
        extras.insert("chdomain".into(), "chat-aa.sooplive.com".into());
        extras.insert("chpt".into(), "8040".into());
        extras.insert("bjid".into(), "example".into());
        p.configure_connection(None, Some(&extras));

        let login = SoopDanmuProtocol::make_packet(SVC_LOGIN, &["", "", "16"]);
        let (tx, mut rx) = mpsc::channel(4);
        let items = p
            .decode_message(&Message::Binary(login), "example", &tx)
            .await
            .expect("decode");
        assert!(items.is_empty());
        let join = rx.try_recv().expect("JOIN packet");
        let Message::Binary(data) = join else {
            panic!("expected binary JOIN");
        };
        let parsed = SoopDanmuProtocol::parse_packets(&data);
        assert_eq!(parsed[0].0, SVC_JOINCH);
        assert_eq!(parsed[0].1[0], "7604");
        assert_eq!(parsed[0].1[1], "ticket");
        assert_ne!(parsed[0].1[0], "example");
    }
}
