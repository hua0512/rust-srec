//! TARS response structures for Huya API

use rustc_hash::FxHashMap;
use tars_codec::{error::TarsError, types::TarsValue};

#[derive(Default, Debug)]
#[allow(dead_code)]
pub struct HuyaGetTokenResp {
    pub url: String,
    pub cdn_type: String,
    pub stream_name: String,
    pub presenter_uid: i64,
    pub anti_code: String,
    pub s_time: String,
    pub flv_anti_code: String,
    pub hls_anti_code: String,
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
            let presenter_uid = take(3)?.try_into_i64()?;
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
use super::stream::BeginLiveNotice;

// StreamSettingNotice from JavaScript x.StreamSettingNotice
// tag 0: lPresenterUid (i64)
// tag 1: iBitRate (i32)
// tag 2: iResolution (i32)
// tag 3: iFrameRate (i32)
// tag 4: lLiveId (i64)
// tag 5: sDisplayName (string)
// tag 6: iScreenType (i32)
// tag 7: sVideoLayout (string)
// tag 8: iLowDelayMode (i32)
#[derive(Default, Debug, Clone)]
#[allow(dead_code)]
pub struct StreamSettingNotice {
    pub l_presenter_uid: i64,   // tag 0
    pub i_bit_rate: i32,        // tag 1
    pub i_resolution: i32,      // tag 2
    pub i_frame_rate: i32,      // tag 3
    pub l_live_id: i64,         // tag 4
    pub s_display_name: String, // tag 5
    pub i_screen_type: i32,     // tag 6
    pub s_video_layout: String, // tag 7
    pub i_low_delay_mode: i32,  // tag 8
}

impl TryFrom<TarsValue> for StreamSettingNotice {
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

            let l_presenter_uid = take_optional_i64(&mut map, 0);
            let i_bit_rate = take_optional_i32(&mut map, 1);
            let i_resolution = take_optional_i32(&mut map, 2);
            let i_frame_rate = take_optional_i32(&mut map, 3);
            let l_live_id = take_optional_i64(&mut map, 4);
            let s_display_name = take_optional_string(&mut map, 5);
            let i_screen_type = take_optional_i32(&mut map, 6);
            let s_video_layout = take_optional_string(&mut map, 7);
            let i_low_delay_mode = take_optional_i32(&mut map, 8);

            Ok(StreamSettingNotice {
                l_presenter_uid,
                i_bit_rate,
                i_resolution,
                i_frame_rate,
                l_live_id,
                s_display_name,
                i_screen_type,
                s_video_layout,
                i_low_delay_mode,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}

// GetLivingInfoRsp from JavaScript x.GetLivingInfoRsp
#[derive(Default, Debug, Clone)]
#[allow(dead_code)]
pub struct GetLivingInfoRsp {
    pub b_is_living: i32,                             // tag 0
    pub t_notice: BeginLiveNotice,                    // tag 1
    pub t_stream_setting_notice: StreamSettingNotice, // tag 2
    pub b_is_self_living: i32,                        // tag 3
    pub s_message: String,                            // tag 4
    pub i_show_title_for_immersion: i32,              // tag 5
}

impl TryFrom<TarsValue> for GetLivingInfoRsp {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
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

            let b_is_living = take_optional_i32(&mut map, 0);

            let t_notice = map
                .remove(&1)
                .and_then(|v| BeginLiveNotice::try_from(v).ok())
                .unwrap_or_default();

            let t_stream_setting_notice = map
                .remove(&2)
                .and_then(|v| StreamSettingNotice::try_from(v).ok())
                .unwrap_or_default();

            let b_is_self_living = take_optional_i32(&mut map, 3);
            let s_message = take_optional_string(&mut map, 4);
            let i_show_title_for_immersion = take_optional_i32(&mut map, 5);

            Ok(GetLivingInfoRsp {
                b_is_living,
                t_notice,
                t_stream_setting_notice,
                b_is_self_living,
                s_message,
                i_show_title_for_immersion,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}
