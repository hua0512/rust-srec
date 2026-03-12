//! TARS response structures for Huya API

use super::stream::BeginLiveNotice;
use rustc_hash::FxHashMap;
use tars_codec::{error::TarsError, types::TarsValue};

// GetCdnTokenExRsp from JavaScript x.GetCdnTokenExRsp
#[derive(Default, Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct GetCdnTokenExRsp {
    pub flv_token: String, // tag 0
    pub expire_time: i32,  // tag 1
}

impl From<GetCdnTokenExRsp> for TarsValue {
    fn from(rsp: GetCdnTokenExRsp) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::String(rsp.flv_token));
        map.insert(1, TarsValue::Int(rsp.expire_time));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for GetCdnTokenExRsp {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(GetCdnTokenExRsp {
            flv_token: take(0)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            expire_time: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

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
#[derive(Default, Debug, Clone, PartialEq)]
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

impl From<StreamSettingNotice> for TarsValue {
    fn from(notice: StreamSettingNotice) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Long(notice.l_presenter_uid));
        map.insert(1, TarsValue::Int(notice.i_bit_rate));
        map.insert(2, TarsValue::Int(notice.i_resolution));
        map.insert(3, TarsValue::Int(notice.i_frame_rate));
        map.insert(4, TarsValue::Long(notice.l_live_id));
        map.insert(5, TarsValue::String(notice.s_display_name));
        map.insert(6, TarsValue::Int(notice.i_screen_type));
        map.insert(7, TarsValue::String(notice.s_video_layout));
        map.insert(8, TarsValue::Int(notice.i_low_delay_mode));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for StreamSettingNotice {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(StreamSettingNotice {
            l_presenter_uid: take(0)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_bit_rate: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_resolution: take(2)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_frame_rate: take(3)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            l_live_id: take(4)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_display_name: take(5)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_screen_type: take(6)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_video_layout: take(7)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_low_delay_mode: take(8)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

// GetLivingInfoRsp from JavaScript x.GetLivingInfoRsp
#[derive(Default, Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct GetLivingInfoRsp {
    pub b_is_living: i32,                             // tag 0
    pub t_notice: BeginLiveNotice,                    // tag 1
    pub t_stream_setting_notice: StreamSettingNotice, // tag 2
    pub b_is_self_living: i32,                        // tag 3
    pub s_message: String,                            // tag 4
    pub i_show_title_for_immersion: i32,              // tag 5
}

impl From<GetLivingInfoRsp> for TarsValue {
    fn from(rsp: GetLivingInfoRsp) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Int(rsp.b_is_living));
        map.insert(1, TarsValue::from(rsp.t_notice));
        map.insert(2, TarsValue::from(rsp.t_stream_setting_notice));
        map.insert(3, TarsValue::Int(rsp.b_is_self_living));
        map.insert(4, TarsValue::String(rsp.s_message));
        map.insert(5, TarsValue::Int(rsp.i_show_title_for_immersion));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for GetLivingInfoRsp {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(GetLivingInfoRsp {
            b_is_living: take(0)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            t_notice: take(1)
                .and_then(|v| BeginLiveNotice::try_from(v).ok())
                .unwrap_or_default(),
            t_stream_setting_notice: take(2)
                .and_then(|v| StreamSettingNotice::try_from(v).ok())
                .unwrap_or_default(),
            b_is_self_living: take(3)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_message: take(4)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_show_title_for_immersion: take(5)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_living_info_rsp_compatibility() {
        let rsp = GetLivingInfoRsp {
            b_is_living: 1,
            t_notice: BeginLiveNotice::default(),
            t_stream_setting_notice: StreamSettingNotice::default(),
            b_is_self_living: 0,
            s_message: "ok".into(),
            i_show_title_for_immersion: 1,
        };

        let tars_val = TarsValue::from(rsp.clone());
        let decoded = GetLivingInfoRsp::try_from(tars_val).unwrap();
        assert_eq!(rsp, decoded);
    }
}
