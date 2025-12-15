//! TARS encoding/decoding functions for Huya API

use bytes::Bytes;
use rustc_hash::FxHashMap;
use tars_codec::{
    de::from_bytes,
    decode_response_zero_copy,
    error::TarsError,
    types::{TarsMessage, TarsRequestHeader, TarsValue},
};

use super::responses::{GetLivingInfoRsp, HuyaGetTokenResp};
use super::types::{GetCdnTokenInfoReq, GetLivingInfoReq, HuyaUserId};

pub fn build_get_living_info_request(presenter_uid: i64, _ua: &str) -> Result<Bytes, TarsError> {
    // TODO: replace those
    let user_id = HuyaUserId::new(
        0,
        String::new(),
        String::new(),
        "huya_nftv&2.5.1.3141&official&30".to_string(),
        String::new(),
        0,
        "android_tv".to_string(),
        String::new(),
    );
    let req = GetLivingInfoReq::new(
        user_id,
        0,             // l_top_sid
        0,             // l_sub_sid
        presenter_uid, // l_presenter_uid
        String::new(), // s_trace_source
        String::new(), // s_password
        0,             // i_room_id
        0,             // i_free_flow_flag
        0,             // i_ip_stack
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
