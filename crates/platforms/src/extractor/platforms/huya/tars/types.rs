//! Basic TARS request types for Huya API

use rustc_hash::FxHashMap;
use tars_codec::{error::TarsError, types::TarsValue};

#[derive(Debug, PartialEq, Clone, Default)]
pub struct WebSocketCommand {
    cmd_type: i32,
    /// Binary data payload - serialized as TARS SimpleList (bytes), not as List of integers
    data: Vec<u8>,
    request_id: i64,
    trace_id: String,
    encrypt_type: i32,
    time: i64,
    md5: String,
}

impl WebSocketCommand {
    pub fn new(
        cmd_type: i32,
        data: Vec<u8>,
        request_id: i64,
        trace_id: String,
        encrypt_type: i32,
        time: i64,
        md5: String,
    ) -> Self {
        Self {
            cmd_type,
            data,
            request_id,
            trace_id,
            encrypt_type,
            time,
            md5,
        }
    }

    /// Returns the command type
    pub fn cmd_type(&self) -> i32 {
        self.cmd_type
    }

    /// Returns a reference to the inner data payload
    pub fn data(&self) -> &Vec<u8> {
        &self.data
    }
}

impl From<WebSocketCommand> for TarsValue {
    fn from(cmd: WebSocketCommand) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Int(cmd.cmd_type));
        map.insert(1, TarsValue::SimpleList(cmd.data.into()));
        map.insert(2, TarsValue::Long(cmd.request_id));
        map.insert(3, TarsValue::String(cmd.trace_id));
        map.insert(4, TarsValue::Int(cmd.encrypt_type));
        map.insert(5, TarsValue::Long(cmd.time));
        map.insert(6, TarsValue::String(cmd.md5));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for WebSocketCommand {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(WebSocketCommand {
            cmd_type: take(0)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            data: take(1)
                .and_then(|v| v.try_into_simple_list().ok())
                .map(|b| b.to_vec())
                .unwrap_or_default(),
            request_id: take(2)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            trace_id: take(3)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            encrypt_type: take(4)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            time: take(5)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            md5: take(6)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
        })
    }
}

/// Push message wrapper for Huya WebSocket push notifications (type 22)
/// Based on Huya's x.WSPushMessage definition
#[derive(Debug, PartialEq, Clone, Default)]
pub struct WsPushMessage {
    /// Push type
    pub e_push_type: i32,
    /// URI identifier for the message type
    pub i_uri: i64,
    /// Message payload data (serialized inner struct)
    pub s_msg: Vec<u8>,
    /// Protocol type
    pub i_protocol_type: i32,
    /// Group identifier (e.g., "live:294636272")
    pub s_group_id: String,
    /// Message ID
    pub l_msg_id: i64,
    /// Message tag
    pub i_msg_tag: i32,
}

impl From<WsPushMessage> for TarsValue {
    fn from(msg: WsPushMessage) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Int(msg.e_push_type));
        map.insert(1, TarsValue::Long(msg.i_uri));
        map.insert(2, TarsValue::SimpleList(msg.s_msg.into()));
        map.insert(3, TarsValue::Int(msg.i_protocol_type));
        map.insert(4, TarsValue::String(msg.s_group_id));
        map.insert(5, TarsValue::Long(msg.l_msg_id));
        map.insert(6, TarsValue::Int(msg.i_msg_tag));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for WsPushMessage {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(WsPushMessage {
            e_push_type: take(0)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_uri: take(1)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_msg: take(2)
                .and_then(|v| v.try_into_simple_list().ok())
                .map(|b| b.to_vec())
                .unwrap_or_default(),
            i_protocol_type: take(3)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_group_id: take(4)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            l_msg_id: take(5)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_msg_tag: take(6)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

/// Sender info nested inside danmu message (URI 1400)
/// Based on Huya's x.SenderInfo definition
#[derive(Debug, PartialEq, Clone, Default)]
pub struct SenderInfo {
    /// User UID
    pub l_uid: i64,
    /// User IM ID
    pub l_imid: i64,
    /// Nickname
    pub s_nick_name: String,
    /// Gender
    pub i_gender: i32,
    /// Avatar URL
    pub s_avatar_url: String,
    /// Noble level
    pub i_noble_level: i32,
    // Skipping tag 6 (NobleLevelInfo struct) for simplicity
    /// GUID
    pub s_guid: String,
    /// HuYa UA
    pub s_huya_ua: String,
    /// User type
    pub i_user_type: i32,
}

impl From<SenderInfo> for TarsValue {
    fn from(info: SenderInfo) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Long(info.l_uid));
        map.insert(1, TarsValue::Long(info.l_imid));
        map.insert(2, TarsValue::String(info.s_nick_name));
        map.insert(3, TarsValue::Int(info.i_gender));
        map.insert(4, TarsValue::String(info.s_avatar_url));
        map.insert(5, TarsValue::Int(info.i_noble_level));
        map.insert(7, TarsValue::String(info.s_guid));
        map.insert(8, TarsValue::String(info.s_huya_ua));
        map.insert(9, TarsValue::Int(info.i_user_type));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for SenderInfo {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(SenderInfo {
            l_uid: take(0)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_imid: take(1)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_nick_name: take(2)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_gender: take(3)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_avatar_url: take(4)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_noble_level: take(5)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_guid: take(7)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_huya_ua: take(8)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_user_type: take(9)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

/// Bullet format struct for danmu message
#[derive(Debug, PartialEq, Clone, Default)]
pub struct BulletFormat {
    /// Color value (RGB as i32)
    pub i_color: i32,
    /// Unknown field
    pub i_unknown1: i32,
    /// Unknown field
    pub i_unknown2: i32,
}

impl From<BulletFormat> for TarsValue {
    fn from(f: BulletFormat) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Int(f.i_color));
        map.insert(1, TarsValue::Int(f.i_unknown1));
        map.insert(2, TarsValue::Int(f.i_unknown2));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for BulletFormat {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(BulletFormat {
            i_color: take(0)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_unknown1: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_unknown2: take(2)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

/// Message notice (danmu message) (URI 1400)
/// Based on Huya's x.MessageNotice definition
#[derive(Debug, PartialEq, Clone, Default)]
pub struct MessageNotice {
    /// User info
    pub t_user_info: SenderInfo,
    /// Top SID
    pub l_tid: i64,
    /// Sub SID
    pub l_sid: i64,
    /// Message content
    pub s_content: String,
    /// Show mode
    pub i_show_mode: i32,
    // Tag 5 is ContentFormat
    /// Bullet format
    pub t_bullet_format: BulletFormat,
    /// Terminal type
    pub i_term_type: i32,
}

impl From<MessageNotice> for TarsValue {
    fn from(msg: MessageNotice) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, msg.t_user_info.into());
        map.insert(1, TarsValue::Long(msg.l_tid));
        map.insert(2, TarsValue::Long(msg.l_sid));
        map.insert(3, TarsValue::String(msg.s_content));
        map.insert(4, TarsValue::Int(msg.i_show_mode));
        map.insert(6, msg.t_bullet_format.into());
        map.insert(7, TarsValue::Int(msg.i_term_type));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for MessageNotice {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(MessageNotice {
            t_user_info: take(0)
                .and_then(|v| SenderInfo::try_from(v).ok())
                .unwrap_or_default(),
            l_tid: take(1)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_sid: take(2)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_content: take(3)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_show_mode: take(4)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            t_bullet_format: take(6)
                .and_then(|v| BulletFormat::try_from(v).ok())
                .unwrap_or_default(),
            i_term_type: take(7)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, PartialEq, Clone, Default)]
pub struct LiveLaunchReq {
    id: HuyaUserId,
    live_ub: LiveUserBase,
    support_domain: bool,
}

impl LiveLaunchReq {
    pub fn new(id: HuyaUserId, live_ub: LiveUserBase, support_domain: bool) -> Self {
        Self {
            id,
            live_ub,
            support_domain,
        }
    }
}

impl From<LiveLaunchReq> for TarsValue {
    fn from(req: LiveLaunchReq) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, req.id.into());
        map.insert(1, req.live_ub.into());
        map.insert(2, TarsValue::Bool(req.support_domain));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for LiveLaunchReq {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(LiveLaunchReq {
            id: take(0)
                .and_then(|v| HuyaUserId::try_from(v).ok())
                .unwrap_or_default(),
            live_ub: take(1)
                .and_then(|v| LiveUserBase::try_from(v).ok())
                .unwrap_or_default(),
            support_domain: take(2)
                .and_then(|v| v.try_into_bool().ok())
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct LiveUserBase {
    e_source: i32,
    e_type: i32,
    ua_ex: LiveAppUAEx,
}

impl LiveUserBase {
    pub fn new(e_source: i32, e_type: i32, ua_ex: LiveAppUAEx) -> Self {
        Self {
            e_source,
            e_type,
            ua_ex,
        }
    }
}

impl From<LiveUserBase> for TarsValue {
    fn from(ub: LiveUserBase) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Int(ub.e_source));
        map.insert(1, TarsValue::Int(ub.e_type));
        map.insert(2, ub.ua_ex.into());
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for LiveUserBase {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(LiveUserBase {
            e_source: take(0)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            e_type: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            ua_ex: take(2)
                .and_then(|v| LiveAppUAEx::try_from(v).ok())
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct LiveAppUAEx {
    s_imei: String,
    s_apn: String,
    s_net_type: String,
    s_device_id: String,
    s_mid: String,
}

impl From<LiveAppUAEx> for TarsValue {
    fn from(ua: LiveAppUAEx) -> Self {
        let mut map = FxHashMap::default();
        map.insert(1, TarsValue::String(ua.s_imei));
        map.insert(2, TarsValue::String(ua.s_apn));
        map.insert(3, TarsValue::String(ua.s_net_type));
        map.insert(4, TarsValue::String(ua.s_device_id));
        map.insert(5, TarsValue::String(ua.s_mid));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for LiveAppUAEx {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(LiveAppUAEx {
            s_imei: take(1)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_apn: take(2)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_net_type: take(3)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_device_id: take(4)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_mid: take(5)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct WsRegisterGroupReq {
    group_id: Vec<String>,
    token: String,
}

impl WsRegisterGroupReq {
    pub fn new(group_id: Vec<String>, token: String) -> Self {
        Self { group_id, token }
    }
}

impl From<WsRegisterGroupReq> for TarsValue {
    fn from(req: WsRegisterGroupReq) -> Self {
        let mut map = FxHashMap::default();
        let group_id_vals = req
            .group_id
            .into_iter()
            .map(|s| Box::new(TarsValue::String(s)))
            .collect();
        map.insert(0, TarsValue::List(group_id_vals));
        map.insert(1, TarsValue::String(req.token));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for WsRegisterGroupReq {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(WsRegisterGroupReq {
            group_id: take(0)
                .and_then(|v| v.try_into_list().ok())
                .map(|list| {
                    list.into_iter()
                        .filter_map(|v| v.try_into_string().ok())
                        .collect()
                })
                .unwrap_or_default(),
            token: take(1)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
        })
    }
}

pub struct GetCdnTokenInfoReq {
    url: String,
    cdn_type: String,
    stream_name: String,
    presenter_uid: i64,
}

impl GetCdnTokenInfoReq {
    pub fn new(url: String, stream_name: String, cdn_type: String, presenter_uid: i64) -> Self {
        Self {
            url,
            cdn_type,
            stream_name,
            presenter_uid,
        }
    }
}

impl From<GetCdnTokenInfoReq> for TarsValue {
    fn from(req: GetCdnTokenInfoReq) -> Self {
        let mut struct_map = FxHashMap::default();
        struct_map.insert(0, TarsValue::String(req.url));
        struct_map.insert(1, TarsValue::String(req.cdn_type));
        struct_map.insert(2, TarsValue::String(req.stream_name));
        struct_map.insert(3, TarsValue::Long(req.presenter_uid));
        TarsValue::Struct(struct_map)
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct HuyaUserId {
    pub l_uid: i64,
    pub s_guid: String,
    pub s_token: String,
    pub s_huya_ua: String,
    pub s_cookie: String,
    pub i_token_type: i32,
    pub s_device_info: String,
    pub s_qimei: String,
}

impl HuyaUserId {
    pub fn new(l_uid: i64, s_guid: String, s_token: String, s_huya_ua: String) -> Self {
        Self {
            l_uid,
            s_guid,
            s_token,
            s_huya_ua,
            ..Default::default()
        }
    }

    pub fn with_cookie(mut self, s_cookie: String) -> Self {
        self.s_cookie = s_cookie;
        self
    }

    pub fn with_device_info(mut self, s_device_info: String) -> Self {
        self.s_device_info = s_device_info;
        self
    }

    pub fn with_qimei(mut self, s_qimei: String) -> Self {
        self.s_qimei = s_qimei;
        self
    }

    pub fn with_token_type(mut self, i_token_type: i32) -> Self {
        self.i_token_type = i_token_type;
        self
    }
}

impl From<HuyaUserId> for TarsValue {
    fn from(req: HuyaUserId) -> Self {
        let mut struct_map = FxHashMap::default();
        struct_map.insert(0, TarsValue::Long(req.l_uid));
        struct_map.insert(1, TarsValue::String(req.s_guid));
        struct_map.insert(2, TarsValue::String(req.s_token));
        struct_map.insert(3, TarsValue::String(req.s_huya_ua));
        struct_map.insert(4, TarsValue::String(req.s_cookie));
        struct_map.insert(5, TarsValue::Int(req.i_token_type));
        struct_map.insert(6, TarsValue::String(req.s_device_info));
        struct_map.insert(7, TarsValue::String(req.s_qimei));
        TarsValue::Struct(struct_map)
    }
}

impl TryFrom<TarsValue> for HuyaUserId {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(HuyaUserId {
            l_uid: take(0)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_guid: take(1)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_token: take(2)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_huya_ua: take(3)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_cookie: take(4)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_token_type: take(5)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_device_info: take(6)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_qimei: take(7)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
        })
    }
}

// x.GetLivingInfoReq from JavaScript
// tag 0: tId (UserId struct)
// tag 1: lTopSid (i64)
// tag 2: lSubSid (i64)
// tag 3: lPresenterUid (i64)
// tag 4: sTraceSource (string)
// tag 5: sPassword (string)
// tag 6: iRoomId (i64)
// tag 7: iFreeFlowFlag (i32)
// tag 8: iIpStack (i32)
#[derive(Default, Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct GetLivingInfoReq {
    pub t_id: HuyaUserId,       // tag 0
    pub l_top_sid: i64,         // tag 1
    pub l_sub_sid: i64,         // tag 2
    pub l_presenter_uid: i64,   // tag 3
    pub s_trace_source: String, // tag 4
    pub s_password: String,     // tag 5
    pub i_room_id: i64,         // tag 6
    pub i_free_flow_flag: i32,  // tag 7
    pub i_ip_stack: i32,        // tag 8
}

impl GetLivingInfoReq {
    pub fn new(t_id: HuyaUserId, l_presenter_uid: i64) -> Self {
        Self {
            t_id,
            l_presenter_uid,
            ..Default::default()
        }
    }

    pub fn with_sid(mut self, l_top_sid: i64, l_sub_sid: i64) -> Self {
        self.l_top_sid = l_top_sid;
        self.l_sub_sid = l_sub_sid;
        self
    }

    pub fn with_source(mut self, s_trace_source: String) -> Self {
        self.s_trace_source = s_trace_source;
        self
    }

    pub fn with_password(mut self, s_password: String) -> Self {
        self.s_password = s_password;
        self
    }

    pub fn with_room_id(mut self, i_room_id: i64) -> Self {
        self.i_room_id = i_room_id;
        self
    }

    pub fn with_free_flow(mut self, i_free_flow_flag: i32) -> Self {
        self.i_free_flow_flag = i_free_flow_flag;
        self
    }

    pub fn with_ip_stack(mut self, i_ip_stack: i32) -> Self {
        self.i_ip_stack = i_ip_stack;
        self
    }
}

impl From<GetLivingInfoReq> for TarsValue {
    fn from(req: GetLivingInfoReq) -> Self {
        let mut struct_map = FxHashMap::default();
        struct_map.insert(0, req.t_id.into());
        struct_map.insert(1, TarsValue::Long(req.l_top_sid));
        struct_map.insert(2, TarsValue::Long(req.l_sub_sid));
        struct_map.insert(3, TarsValue::Long(req.l_presenter_uid));
        struct_map.insert(4, TarsValue::String(req.s_trace_source));
        struct_map.insert(5, TarsValue::String(req.s_password));
        struct_map.insert(6, TarsValue::Long(req.i_room_id));
        struct_map.insert(7, TarsValue::Int(req.i_free_flow_flag));
        struct_map.insert(8, TarsValue::Int(req.i_ip_stack));
        TarsValue::Struct(struct_map)
    }
}

impl TryFrom<TarsValue> for GetLivingInfoReq {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(GetLivingInfoReq {
            t_id: take(0)
                .and_then(|v| HuyaUserId::try_from(v).ok())
                .unwrap_or_default(),
            l_top_sid: take(1)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_sub_sid: take(2)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_presenter_uid: take(3)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_trace_source: take(4)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_password: take(5)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_room_id: take(6)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_free_flow_flag: take(7)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_ip_stack: take(8)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_huya_user_id_compatibility() {
        let user_id = HuyaUserId::new(123, "guid".into(), "token".into(), "ua".into())
            .with_cookie("cookie".into())
            .with_token_type(1)
            .with_device_info("device".into())
            .with_qimei("qimei".into());

        let tars_val = TarsValue::from(user_id.clone());
        let decoded = HuyaUserId::try_from(tars_val).unwrap();
        assert_eq!(user_id, decoded);
    }

    #[test]
    fn test_get_living_info_req_compatibility() {
        let req = GetLivingInfoReq::new(HuyaUserId::default(), 300)
            .with_sid(100, 200)
            .with_source("source".into())
            .with_password("pass".into())
            .with_room_id(400)
            .with_free_flow(1)
            .with_ip_stack(2);

        let tars_val = TarsValue::from(req.clone());
        let decoded = GetLivingInfoReq::try_from(tars_val).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_websocket_command_compatibility() {
        let cmd = WebSocketCommand::new(
            1,
            vec![1, 2, 3],
            456,
            "trace".into(),
            0,
            123456789,
            "md5".into(),
        );

        let tars_val = TarsValue::from(cmd.clone());
        let decoded = WebSocketCommand::try_from(tars_val).unwrap();
        assert_eq!(cmd, decoded);
    }
}
