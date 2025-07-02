use ahash::AHashMap;
use bytes::Bytes;
use tars_codec::{
    de::{TarsDeserializer, from_bytes},
    decode_response_from_bytes, decode_response_zero_copy,
    error::TarsError,
    types::{TarsMessage, TarsRequestHeader, TarsValue},
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
        let mut struct_map = AHashMap::new();
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
    let mut body = AHashMap::new();
    let tars_value: TarsValue = req.into();
    body.insert(
        "tReq".to_string(),
        tars_codec::ser::to_bytes_mut(&tars_value)?,
    );

    let message = TarsMessage {
        header: TarsRequestHeader {
            version: 3,
            packet_type: 0,
            message_type: 0,
            request_id: 1,
            servant_name: "liveui".to_string(),
            func_name: "getCdnTokenInfo".to_string(),
            timeout: 0,
            context: AHashMap::new(),
            status: AHashMap::new(),
        },
        body,
    };

    tars_codec::encode_request(message).map(|bytes| bytes.to_vec())
}

impl TryFrom<TarsValue> for HuyaGetTokenResp {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
            let mut take = |tag: u8| -> Result<TarsValue, TarsError> {
                map.remove(&tag).ok_or(TarsError::TagNotFound(tag))
            };

            let url = take(0)?.try_into_string()?;
            let cdn_type = take(1)?.try_into_string()?;
            let stream_name = take(2)?.try_into_string()?;
            let presenter_uid = take(3)?.try_into_i32()?;
            let anti_code = take(4)?.try_into_string()?;
            let s_time = take(5)?.try_into_string()?;
            let flv_anti_code = take(6)?.try_into_string()?;
            let hls_anti_code = take(7)?.try_into_string()?;

            Ok(HuyaGetTokenResp {
                url,
                cdn_type,
                stream_name,
                presenter_uid,
                anti_code,
                s_time,
                flv_anti_code,
                hls_anti_code,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}

pub fn decode_get_cdn_token_info_response(
    bytes: Bytes,
) -> Result<HuyaGetTokenResp, tars_codec::error::TarsError> {
    let message = decode_response_zero_copy(bytes)?;
    // println!("Message: {:?}", message);
    let resp_bytes = message.body.get("tRsp").ok_or(TarsError::Unknown)?;
    // println!("Resp Bytes: {:?}", resp_bytes);
    let tars_value = from_bytes(resp_bytes.clone())?;
    HuyaGetTokenResp::try_from(tars_value)
}

// Response Structures
#[derive(Default, Debug)]
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
