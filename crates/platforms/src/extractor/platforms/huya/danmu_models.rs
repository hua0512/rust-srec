/// Huya message URI constants and packet structures.
#[allow(dead_code)]

/// Huya message URI constants for different message types.
pub mod huya_uri {
    /// Chat message (弹幕消息)
    pub const MESSAGE_NOTICE: i32 = 1400;
    /// Gift message (礼物消息)
    pub const SEND_ITEM_SUB_BROADCAST: i32 = 6501;
    /// Noble enter notification (贵族进场)
    pub const NOBLE_ENTER_NOTICE: i32 = 6502;
    /// VIP enter notification (VIP进场)
    pub const VIP_ENTER_BANNER: i32 = 6503;
    /// User enter notification (用户进场)
    pub const USER_ENTER_NOTICE: i32 = 6504;
}

/// Huya WebSocket command types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HuyaWsCmd {
    /// Heartbeat request (client to server)
    HeartbeatReq = 1,
    /// Heartbeat response (server to client)
    HeartbeatRsp = 2,
    /// Register request (client to server)
    RegisterReq = 3,
    /// Register response (server to client)
    RegisterRsp = 4,
    /// Message push request (server to client)
    MsgPushReq = 5,
    /// WUP response (server to client)
    WupRsp = 7,

    /// Register group request (client to server)
    RegisterGroupReq = 16,
    /// Register group response (server to client)
    RegisterGroupRsp = 17,

    /// Push message (danmu, gifts, etc.) (server to client)
    PushMessage = 22,
}

/// Huya source type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HuyaSourceType {
    /// PC web
    PcWeb = 3,
}
