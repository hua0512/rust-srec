use bytes::Bytes;
use rustc_hash::FxHashMap;
use tars_codec::{
    de::from_bytes,
    decode_response_zero_copy,
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
        let mut struct_map = FxHashMap::default();
        struct_map.insert(0, TarsValue::String(req.url));
        struct_map.insert(1, TarsValue::String(req.cdn_type));
        struct_map.insert(2, TarsValue::String(req.stream_name));
        struct_map.insert(3, TarsValue::Int(req.presenter_uid));
        TarsValue::Struct(struct_map)
    }
}

pub fn build_get_cdn_token_info_request(
    stream_name: &str,
    cdn_type: &str,
    presenter_uid: i32,
) -> Result<Bytes, tars_codec::error::TarsError> {
    let req = GetCdnTokenInfoReq::new(
        String::new(),
        stream_name.to_owned(),
        cdn_type.to_owned(),
        presenter_uid,
    );
    let mut body = FxHashMap::default();
    let tars_value: TarsValue = req.into();
    body.insert(
        String::from("tReq"),
        tars_codec::ser::to_bytes_mut(&tars_value)?,
    );

    let message = TarsMessage {
        header: TarsRequestHeader {
            version: 3,
            packet_type: 0,
            message_type: 0,
            request_id: 1,
            servant_name: String::from("liveui"),
            func_name: String::from("getCdnTokenInfo"),
            timeout: 0,
            context: FxHashMap::default(),
            status: FxHashMap::default(),
        },
        body,
    };

    let bytes = tars_codec::encode_request(&message)?;
    Ok(bytes.freeze())
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
#[allow(dead_code)]
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
