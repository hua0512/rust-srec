//! Bilibili notification (`op = 5`) command parsing (chat, gifts, super chat, control events).

use std::borrow::Cow;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use prost::Message as ProstMessage;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use crate::danmaku::{DanmuControlEvent, DanmuItem, DanmuMessage};

// Borrow the Base64 payload and skip unrelated JSON fields on the common V2
// path. Cow still accepts escaped strings, allocating only when unescaping.
#[derive(Deserialize)]
struct GiftV2Notification<'a> {
    #[serde(borrow)]
    cmd: Cow<'a, str>,
    #[serde(borrow)]
    data: GiftV2Payload<'a>,
}

#[derive(Deserialize)]
struct GiftV2Payload<'a> {
    #[serde(borrow)]
    pb: Cow<'a, str>,
}

// Subset of SEND_GIFT_V2 used for recording. Tags follow the schema shipped in
// blive-message-listener@0.5.6-beta.1 (src/protobuf/SEND_GIFT_V2.proto.ts).
// Prost skips other fields, including nested medal and animation information.
#[derive(Clone, PartialEq, prost::Message)]
pub(super) struct SendGiftV2 {
    #[prost(int64, tag = "1")]
    pub uid: i64,
    #[prost(string, tag = "2")]
    pub uname: String,
    #[prost(message, repeated, tag = "10")]
    pub gift_list: Vec<GiftV2>,
    #[prost(bool, tag = "11")]
    pub switch: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub(super) struct GiftV2 {
    #[prost(string, tag = "2")]
    pub gift_name: String,
    #[prost(int64, tag = "3")]
    pub num: i64,
    #[prost(int64, tag = "5")]
    pub price: i64,
    #[prost(string, tag = "8")]
    pub coin_type: String,
    #[prost(int64, tag = "10")]
    pub timestamp: i64,
}

/// Commands `parse_notification` turns into items; everything else is dropped.
fn handles_command(cmd_base: &str) -> bool {
    matches!(
        cmd_base,
        "DANMU_MSG"
            | "DANMU_MSG_MIRROR"
            | "SEND_GIFT"
            | "SEND_GIFT_V2"
            | "SUPER_CHAT_MESSAGE"
            | "ROOM_CHANGE"
            | "ROOM_LOCK"
            | "CUT_OFF"
    )
}

/// Append items from a notification (op=5); V2 gifts can contain a batch.
pub(super) fn parse_notification(body: &[u8], items: &mut Vec<DanmuItem>) {
    // Bilibili serialises `cmd` as the first key. Most notifications
    // (INTERACT_WORD, ONLINE_RANK_*, WATCHED_CHANGE, STOP_LIVE_ROOM_LIST,
    // ...) are dropped anyway, so when that layout holds and the command
    // is not handled, skip the frame before it becomes a `Value` tree.
    // Any other layout, or a command with escapes, takes the full parse.
    if let Some(rest) = body.strip_prefix(b"{\"cmd\":\"")
        && let Some(end) = rest.iter().position(|&b| b == b'"')
        && let Ok(cmd) = std::str::from_utf8(&rest[..end])
        && !cmd.contains('\\')
    {
        let cmd_base = cmd.split(':').next().unwrap_or(cmd);
        if !handles_command(cmd_base) {
            return;
        }
        if cmd_base == "SEND_GIFT_V2"
            && let Ok(notification) = serde_json::from_slice::<GiftV2Notification<'_>>(body)
            && notification.cmd.split(':').next() == Some("SEND_GIFT_V2")
        {
            parse_gifts_v2(&notification.data.pb, items);
            return;
        }
        // Duplicate fields or an unexpected envelope use Value's existing
        // semantics below, including keeping the last duplicate field.
    }

    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(cmd) = json.get("cmd").and_then(Value::as_str) else {
        return;
    };

    // Handle DANMU_MSG variants (e.g., "DANMU_MSG:4:0:2:2:2:0")
    let cmd_base = cmd.split(':').next().unwrap_or(cmd);

    let item = match cmd_base {
        "DANMU_MSG" | "DANMU_MSG_MIRROR" => parse_danmu_msg(&json).map(DanmuItem::Message),
        "SEND_GIFT" => parse_gift(&json).map(DanmuItem::Message),
        "SEND_GIFT_V2" => {
            if let Some(pb) = json
                .get("data")
                .and_then(|data| data.get("pb"))
                .and_then(Value::as_str)
            {
                parse_gifts_v2(pb, items);
            }
            return;
        }
        "SUPER_CHAT_MESSAGE" => parse_super_chat(&json).map(DanmuItem::Message),
        "ROOM_CHANGE" => parse_room_change(&json),
        "ROOM_LOCK" | "CUT_OFF" => parse_stream_closed(cmd_base, &json),
        _ => None,
    };
    items.extend(item);
}

fn parse_gifts_v2(pb: &str, items: &mut Vec<DanmuItem>) {
    let bytes = match STANDARD.decode(pb) {
        Ok(bytes) => bytes,
        Err(error) => {
            debug!(%error, "Invalid SEND_GIFT_V2 base64 payload");
            return;
        }
    };
    let batch = match SendGiftV2::decode(bytes.as_slice()) {
        Ok(batch) => batch,
        Err(error) => {
            debug!(%error, "Invalid SEND_GIFT_V2 protobuf payload");
            return;
        }
    };
    // Disabled batches must not produce gifts, matching the upstream gate.
    // Record sender identity as delivered: anonymous senders can have uid=0.
    if !batch.switch || batch.gift_list.is_empty() {
        return;
    }

    let gift_count = batch.gift_list.len();
    items.reserve(gift_count);
    let mut user_id = batch.uid.to_string();
    let mut username = batch.uname;
    for (index, gift) in batch.gift_list.into_iter().enumerate() {
        let Ok(count) = u32::try_from(gift.num) else {
            debug!("Ignoring SEND_GIFT_V2 gift with invalid count");
            continue;
        };
        let Ok(price) = u64::try_from(gift.price) else {
            debug!("Ignoring SEND_GIFT_V2 gift with invalid price");
            continue;
        };
        if count == 0
            || gift.gift_name.is_empty()
            || !matches!(gift.coin_type.as_str(), "gold" | "silver")
        {
            debug!("Ignoring SEND_GIFT_V2 gift with invalid name, count or coin type");
            continue;
        }
        let mut message = DanmuMessage::gift(
            uuid::Uuid::new_v4().to_string(),
            if index + 1 == gift_count {
                std::mem::take(&mut user_id)
            } else {
                user_id.clone()
            },
            if index + 1 == gift_count {
                std::mem::take(&mut username)
            } else {
                username.clone()
            },
            gift.gift_name,
            count,
        )
        .with_metadata("price", serde_json::json!(price));

        let timestamp = if gift.timestamp > 1_000_000_000_000 {
            jiff::Timestamp::from_millisecond(gift.timestamp).ok()
        } else if gift.timestamp > 0 {
            jiff::Timestamp::from_second(gift.timestamp).ok()
        } else {
            None
        };
        if let Some(timestamp) = timestamp {
            message = message.with_timestamp(timestamp);
        }
        items.push(DanmuItem::Message(message));
    }
}

fn parse_stream_closed(cmd: &str, json: &Value) -> Option<DanmuItem> {
    let data = json.get("data");

    let message = data
        .and_then(|d| {
            d.get("message")
                .or_else(|| d.get("msg"))
                .or_else(|| d.get("reason"))
                .or_else(|| d.get("text"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| Some(cmd.to_string()));

    Some(DanmuItem::Control(DanmuControlEvent::StreamClosed {
        message,
        action: None,
    }))
}

fn parse_room_change(json: &Value) -> Option<DanmuItem> {
    let data = json.get("data")?;

    let title = data
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let category = data
        .get("area_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let parent_category = data
        .get("parent_area_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    Some(DanmuItem::Control(DanmuControlEvent::RoomInfoChanged {
        title,
        category,
        parent_category,
    }))
}

pub(super) fn parse_danmu_msg(json: &Value) -> Option<DanmuMessage> {
    let info = json.get("info")?.as_array()?;

    // info[1] = content
    let content = info.get(1)?.as_str()?.to_string();

    // info[2][0] = uid, info[2][1] = name
    let user_info = info.get(2)?.as_array()?;
    let uid = user_info.first()?.as_u64().unwrap_or(0);
    let name = user_info.get(1)?.as_str().unwrap_or("").to_string();

    // info[0][3] = color
    let meta = info.first()?.as_array()?;
    let color = meta
        .get(3)
        .and_then(|v| v.as_u64())
        .map(|c| format!("#{:06X}", c as u32));

    // Check for emoticon in extra field
    let content = if let Some(extra_obj) = meta.get(15)
        && let Some(extra_str) = extra_obj.get("extra").and_then(|v| v.as_str())
        && let Ok(extra) = serde_json::from_str::<Value>(extra_str)
        && let Some(emoticon) = extra.get("emoticon_unique").and_then(|v| v.as_str())
        && !emoticon.is_empty()
    {
        format!("[表情:{}]", emoticon)
    } else {
        content
    };

    let mut danmu = DanmuMessage::chat(
        uuid::Uuid::new_v4().to_string(),
        uid.to_string(),
        name,
        content,
    );

    if let Some(c) = color {
        danmu = danmu.with_color(c);
    }

    Some(danmu)
}

fn parse_gift(json: &Value) -> Option<DanmuMessage> {
    let data = json.get("data")?;

    let name = data.get("uname")?.as_str()?.to_string();
    let uid = data.get("uid")?.as_u64()?;
    let gift_name = data.get("giftName")?.as_str()?.to_string();
    let num = data.get("num").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

    let price = data
        .get("price")
        .or_else(|| data.get("total_coin"))
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .unwrap_or(0);

    let timestamp_ms = data
        .get("timestamp")
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
        .map(|ts| {
            if ts > 1_000_000_000_000 {
                ts
            } else {
                ts * 1000
            }
        });

    let mut msg = DanmuMessage::gift(
        uuid::Uuid::new_v4().to_string(),
        uid.to_string(),
        name,
        gift_name,
        num,
    )
    .with_metadata("price", serde_json::json!(price));

    if let Some(ts_ms) = timestamp_ms
        && let Some(dt) = jiff::Timestamp::from_millisecond(ts_ms).ok()
    {
        msg = msg.with_timestamp(dt);
    }

    Some(msg)
}

fn parse_super_chat(json: &Value) -> Option<DanmuMessage> {
    let data = json.get("data")?;

    let user_info = data.get("user_info")?;
    let name = user_info.get("uname")?.as_str()?.to_string();
    let uid = data.get("uid")?.as_u64()?;
    let content = data.get("message")?.as_str()?.to_string();
    let price = data
        .get("price")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .unwrap_or(0);

    let keep_time = data
        .get("time")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .unwrap_or(0);

    let timestamp_ms = data
        .get("ts")
        .or_else(|| data.get("timestamp"))
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
        .map(|ts| {
            if ts > 1_000_000_000_000 {
                ts
            } else {
                ts * 1000
            }
        });

    let mut msg = DanmuMessage::super_chat(
        uuid::Uuid::new_v4().to_string(),
        uid.to_string(),
        name,
        content,
        price,
    )
    .with_super_chat_keep_time(keep_time);

    if let Some(ts_ms) = timestamp_ms
        && let Some(dt) = jiff::Timestamp::from_millisecond(ts_ms).ok()
    {
        msg = msg.with_timestamp(dt);
    }

    Some(msg)
}
