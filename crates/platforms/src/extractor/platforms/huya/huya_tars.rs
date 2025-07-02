use std::collections::BTreeMap;
use tars_codec::{
    TarsMessage,
    ser::to_bytes,
    types::{TarsRequestHeader, TarsValue},
};

pub struct GetCdnTokenInfoReq {
    url: String,
    cdn_type: String,
    stream_name: String,
    presenter_uid: i32,
}

impl GetCdnTokenInfoReq {
    pub fn new(url: String, stream_name: String, cdn_type: String, presenter_uid: i32) -> Self {
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
        let mut struct_map = BTreeMap::new();
        struct_map.insert(0, TarsValue::String(req.url));
        struct_map.insert(1, TarsValue::String(req.cdn_type));
        struct_map.insert(2, TarsValue::String(req.stream_name));
        struct_map.insert(3, TarsValue::Int(req.presenter_uid));
        TarsValue::Struct(struct_map)
    }
}

pub fn build_get_cdn_token_info_request(
    stream_name: String,
    cdn_type: String,
    presenter_uid: i32,
) -> Result<Vec<u8>, tars_codec::error::TarsError> {
    let req = GetCdnTokenInfoReq::new("".to_string(), stream_name, cdn_type, presenter_uid);
    let mut body = BTreeMap::new();
    let tars_value: TarsValue = req.into();
    body.insert("tReq".to_string(), to_bytes(&tars_value)?);

    let message = TarsMessage {
        header: TarsRequestHeader {
            version: 3,
            packet_type: 0,
            message_type: 0,
            request_id: 1,
            servant_name: "liveui".to_string(),
            func_name: "getCdnTokenInfo".to_string(),
            timeout: 0,
            context: BTreeMap::new(),
            status: BTreeMap::new(),
        },
        body,
    };

    tars_codec::encode_request(message).map(|bytes| bytes.to_vec())
}

// Response Structures
pub struct HuyaGetTokenResp {
    pub url: String,
    pub cdn_type: String,
    pub stream_name: String,
    pub presenter_uid: i32,
    pub anti_code: String,
    pub s_time: String,
    pub flv_anti_code: String,
    pub hls_anti_code: String,
}

impl TryFrom<TarsValue> for HuyaGetTokenResp {
    type Error = ();

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
            Ok(Self {
                url: map
                    .remove(&0)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                cdn_type: map
                    .remove(&1)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                stream_name: map
                    .remove(&2)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                presenter_uid: map
                    .remove(&3)
                    .and_then(|v| {
                        if let TarsValue::Int(i) = v {
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                anti_code: map
                    .remove(&4)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                s_time: map
                    .remove(&5)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                flv_anti_code: map
                    .remove(&6)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                hls_anti_code: map
                    .remove(&7)
                    .and_then(|v| {
                        if let TarsValue::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
            })
        } else {
            Err(())
        }
    }
}
