//! TARS encoding/decoding functions for Huya API

use bytes::Bytes;
use rustc_hash::FxHashMap;
use tars_codec::{
    de::from_bytes,
    decode_response_zero_copy,
    error::TarsError,
    next_request_id,
    types::{TarsMessage, TarsRequestHeader, TarsValue},
};

use super::responses::{GetLivingInfoRsp, HuyaGetTokenResp};
use super::types::{GetCdnTokenInfoReq, GetLivingInfoReq, HuyaUserId};

pub(crate) fn build_get_living_info_request(
    presenter_uid: i64,
    ua: &str,
    device: &str,
) -> Result<Bytes, TarsError> {
    let user_id = HuyaUserId::new(0, String::new(), String::new(), ua.to_string())
        .with_device_info(device.to_string());
    let req = GetLivingInfoReq::new(user_id, presenter_uid);
    let mut body = FxHashMap::default();
    body.insert(
        String::from("tReq"),
        tars_codec::ser::to_bytes_mut_wrapped(&TarsValue::from(req))?,
    );

    let message = TarsMessage {
        header: TarsRequestHeader {
            version: 3,
            packet_type: 0,
            message_type: 0,
            request_id: next_request_id(),
            servant_name: String::from("huyaliveui"),
            func_name: String::from("getLivingInfo"),
            timeout: 0,
            context: FxHashMap::default(),
            status: FxHashMap::default(),
        },
        body,
    };

    let bytes = tars_codec::encode_request(&message)?;
    Ok(bytes.freeze())
}

pub fn build_get_cdn_token_info_request(
    stream_name: &str,
    cdn_type: &str,
    presenter_uid: i64,
) -> Result<Bytes, TarsError> {
    let req = GetCdnTokenInfoReq::new(
        String::new(),
        stream_name.to_owned(),
        cdn_type.to_owned(),
        presenter_uid,
    );
    let mut body = FxHashMap::default();
    body.insert(
        String::from("tReq"),
        tars_codec::ser::to_bytes_mut_wrapped(&TarsValue::from(req))?,
    );

    let message = TarsMessage {
        header: TarsRequestHeader {
            version: 3,
            packet_type: 0,
            message_type: 0,
            request_id: next_request_id(),
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

/// Build getLivingInfo request using room ID instead of presenter UID
/// This allows querying stream info without knowing the presenter UID beforehand
pub(crate) fn build_get_living_info_by_room_id_request(
    room_id: i64,
    ua: &str,
    device: &str,
) -> Result<Bytes, TarsError> {
    let user_id = HuyaUserId::new(0, String::new(), String::new(), ua.to_string())
        .with_device_info(device.to_string());
    let req = GetLivingInfoReq::new(user_id, 0).with_room_id(room_id);
    let mut body = FxHashMap::default();
    body.insert(
        String::from("tReq"),
        tars_codec::ser::to_bytes_mut_wrapped(&TarsValue::from(req))?,
    );

    let message = TarsMessage {
        header: TarsRequestHeader {
            version: 3,
            packet_type: 0,
            message_type: 0,
            request_id: next_request_id(),
            servant_name: String::from("huyaliveui"),
            func_name: String::from("getLivingInfo"),
            timeout: 0,
            context: FxHashMap::default(),
            status: FxHashMap::default(),
        },
        body,
    };

    let bytes = tars_codec::encode_request(&message)?;
    Ok(bytes.freeze())
}

pub fn decode_get_living_info_response(bytes: Bytes) -> Result<GetLivingInfoRsp, TarsError> {
    let message = decode_response_zero_copy(bytes)?;
    let resp_bytes = message.body.get("tRsp").ok_or(TarsError::Unknown)?;
    let tars_value = from_bytes(resp_bytes.clone())?;
    GetLivingInfoRsp::try_from(tars_value)
}

pub fn decode_get_cdn_token_info_response(bytes: Bytes) -> Result<HuyaGetTokenResp, TarsError> {
    let message = decode_response_zero_copy(bytes)?;
    let resp_bytes = message.body.get("tRsp").ok_or(TarsError::Unknown)?;
    let tars_value = from_bytes(resp_bytes.clone())?;
    HuyaGetTokenResp::try_from(tars_value)
}
