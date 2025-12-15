//! Basic TARS request types for Huya API

use rustc_hash::FxHashMap;
use tars_codec::{error::TarsError, types::TarsValue};

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

//         this.lUid = 0,
//         this.sGuid = "",
//         this.sToken = "",
//         this.sHuYaUA = "",
//         this.sCookie = "",
//         this.iTokenType = 0,
//         this.sDeviceInfo = "",
//         this.sQIMEI = ""
#[derive(Default, Debug, Clone)]
#[allow(non_snake_case)]
pub struct HuyaUserId {
    pub lUid: i64,
    pub sGuid: String,
    pub sToken: String,
    pub sHuYaUA: String,
    pub sCookie: String,
    pub iTokenType: i32,
    pub sDeviceInfo: String,
    pub sQIMEI: String,
}

impl HuyaUserId {
    #[allow(non_snake_case)]
    pub fn new(
        lUid: i64,
        sGuid: String,
        sToken: String,
        sHuYaUA: String,
        sCookie: String,
        iTokenType: i32,
        sDeviceInfo: String,
        sQIMEI: String,
    ) -> Self {
        Self {
            lUid,
            sGuid,
            sToken,
            sHuYaUA,
            sCookie,
            iTokenType,
            sDeviceInfo,
            sQIMEI,
        }
    }
}

impl From<HuyaUserId> for TarsValue {
    fn from(req: HuyaUserId) -> Self {
        let mut struct_map = FxHashMap::default();
        struct_map.insert(0, TarsValue::Long(req.lUid));
        struct_map.insert(1, TarsValue::String(req.sGuid));
        struct_map.insert(2, TarsValue::String(req.sToken));
        struct_map.insert(3, TarsValue::String(req.sHuYaUA));
        struct_map.insert(4, TarsValue::String(req.sCookie));
        struct_map.insert(5, TarsValue::Int(req.iTokenType));
        struct_map.insert(6, TarsValue::String(req.sDeviceInfo));
        struct_map.insert(7, TarsValue::String(req.sQIMEI));
        TarsValue::Struct(struct_map)
    }
}

impl TryFrom<TarsValue> for HuyaUserId {
    type Error = TarsError;

    #[allow(non_snake_case)]
    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
            let mut take = |tag: u8| -> Result<TarsValue, TarsError> {
                map.remove(&tag).ok_or(TarsError::TagNotFound(tag))
            };

            let lUid = take(0)?.try_into_i64()?;
            let sGuid = take(1)?.try_into_string()?;
            let sToken = take(2)?.try_into_string()?;
            let sHuYaUA = take(3)?.try_into_string()?;
            let sCookie = take(4)?.try_into_string()?;
            let iTokenType = take(5)?.try_into_i32()?;
            let sDeviceInfo = take(6)?.try_into_string()?;
            let sQIMEI = take(7)?.try_into_string()?;

            Ok(HuyaUserId {
                lUid,
                sGuid,
                sToken,
                sHuYaUA,
                sCookie,
                iTokenType,
                sDeviceInfo,
                sQIMEI,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
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
#[derive(Default, Debug, Clone)]
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
    pub fn new(
        t_id: HuyaUserId,
        l_top_sid: i64,
        l_sub_sid: i64,
        l_presenter_uid: i64,
        s_trace_source: String,
        s_password: String,
        i_room_id: i64,
        i_free_flow_flag: i32,
        i_ip_stack: i32,
    ) -> Self {
        Self {
            t_id,
            l_top_sid,
            l_sub_sid,
            l_presenter_uid,
            s_trace_source,
            s_password,
            i_room_id,
            i_free_flow_flag,
            i_ip_stack,
        }
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
        if let TarsValue::Struct(mut map) = value {
            let take_optional_i64 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i64 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i64().ok())
                    .unwrap_or_default()
            };
            let take_optional_i32 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i32 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i32().ok())
                    .unwrap_or_default()
            };
            let take_optional_string = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> String {
                map.remove(&tag)
                    .and_then(|v| v.try_into_string().ok())
                    .unwrap_or_default()
            };

            let t_id = map
                .remove(&0)
                .and_then(|v| HuyaUserId::try_from(v).ok())
                .unwrap_or_default();
            let l_top_sid = take_optional_i64(&mut map, 1);
            let l_sub_sid = take_optional_i64(&mut map, 2);
            let l_presenter_uid = take_optional_i64(&mut map, 3);
            let s_trace_source = take_optional_string(&mut map, 4);
            let s_password = take_optional_string(&mut map, 5);
            let i_room_id = take_optional_i64(&mut map, 6);
            let i_free_flow_flag = take_optional_i32(&mut map, 7);
            let i_ip_stack = take_optional_i32(&mut map, 8);

            Ok(GetLivingInfoReq {
                t_id,
                l_top_sid,
                l_sub_sid,
                l_presenter_uid,
                s_trace_source,
                s_password,
                i_room_id,
                i_free_flow_flag,
                i_ip_stack,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}
