//! Stream info structures for Huya live streaming

use rustc_hash::FxHashMap;
use tars_codec::{error::TarsError, types::TarsValue};

// StreamInfo struct from JavaScript x.StreamInfo
#[derive(Default, Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct StreamInfo {
    pub s_cdn_type: String,                     // tag 0
    pub i_is_master: i32,                       // tag 1
    pub l_channel_id: i64,                      // tag 2
    pub l_sub_channel_id: i64,                  // tag 3
    pub l_presenter_uid: i64,                   // tag 4
    pub s_stream_name: String,                  // tag 5
    pub s_flv_url: String,                      // tag 6
    pub s_flv_url_suffix: String,               // tag 7
    pub s_flv_anti_code: String,                // tag 8
    pub s_hls_url: String,                      // tag 9
    pub s_hls_url_suffix: String,               // tag 10
    pub s_hls_anti_code: String,                // tag 11
    pub i_line_index: i32,                      // tag 12
    pub i_is_multi_stream: i32,                 // tag 13
    pub i_pc_priority_rate: i32,                // tag 14
    pub i_web_priority_rate: i32,               // tag 15
    pub i_mobile_priority_rate: i32,            // tag 16
    pub v_flv_ip_list: Vec<String>,             // tag 17
    pub i_is_p2p_support: i32,                  // tag 18
    pub s_p2p_url: String,                      // tag 19
    pub s_p2p_url_suffix: String,               // tag 20
    pub s_p2p_anti_code: String,                // tag 21
    pub l_free_flag: i64,                       // tag 22
    pub i_is_hevc_support: i32,                 // tag 23
    pub v_p2p_ip_list: Vec<String>,             // tag 24
    pub mp_ext_args: FxHashMap<String, String>, // tag 25
    pub l_timespan: i64,                        // tag 26
    pub l_update_time: i64,                     // tag 27
}

impl From<StreamInfo> for TarsValue {
    fn from(info: StreamInfo) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::String(info.s_cdn_type));
        map.insert(1, TarsValue::Int(info.i_is_master));
        map.insert(2, TarsValue::Long(info.l_channel_id));
        map.insert(3, TarsValue::Long(info.l_sub_channel_id));
        map.insert(4, TarsValue::Long(info.l_presenter_uid));
        map.insert(5, TarsValue::String(info.s_stream_name));
        map.insert(6, TarsValue::String(info.s_flv_url));
        map.insert(7, TarsValue::String(info.s_flv_url_suffix));
        map.insert(8, TarsValue::String(info.s_flv_anti_code));
        map.insert(9, TarsValue::String(info.s_hls_url));
        map.insert(10, TarsValue::String(info.s_hls_url_suffix));
        map.insert(11, TarsValue::String(info.s_hls_anti_code));
        map.insert(12, TarsValue::Int(info.i_line_index));
        map.insert(13, TarsValue::Int(info.i_is_multi_stream));
        map.insert(14, TarsValue::Int(info.i_pc_priority_rate));
        map.insert(15, TarsValue::Int(info.i_web_priority_rate));
        map.insert(16, TarsValue::Int(info.i_mobile_priority_rate));
        map.insert(
            17,
            TarsValue::List(
                info.v_flv_ip_list
                    .into_iter()
                    .map(|s| Box::new(TarsValue::String(s)))
                    .collect(),
            ),
        );
        map.insert(18, TarsValue::Int(info.i_is_p2p_support));
        map.insert(19, TarsValue::String(info.s_p2p_url));
        map.insert(20, TarsValue::String(info.s_p2p_url_suffix));
        map.insert(21, TarsValue::String(info.s_p2p_anti_code));
        map.insert(22, TarsValue::Long(info.l_free_flag));
        map.insert(23, TarsValue::Int(info.i_is_hevc_support));
        map.insert(
            24,
            TarsValue::List(
                info.v_p2p_ip_list
                    .into_iter()
                    .map(|s| Box::new(TarsValue::String(s)))
                    .collect(),
            ),
        );
        map.insert(
            25,
            TarsValue::Map(
                info.mp_ext_args
                    .into_iter()
                    .map(|(k, v)| (TarsValue::String(k), TarsValue::String(v)))
                    .collect(),
            ),
        );
        map.insert(26, TarsValue::Long(info.l_timespan));
        map.insert(27, TarsValue::Long(info.l_update_time));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for StreamInfo {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(StreamInfo {
            s_cdn_type: take(0)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_is_master: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            l_channel_id: take(2)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_sub_channel_id: take(3)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_presenter_uid: take(4)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_stream_name: take(5)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_flv_url: take(6)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_flv_url_suffix: take(7)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_flv_anti_code: take(8)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_hls_url: take(9)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_hls_url_suffix: take(10)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_hls_anti_code: take(11)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_line_index: take(12)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_is_multi_stream: take(13)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_pc_priority_rate: take(14)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_web_priority_rate: take(15)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_mobile_priority_rate: take(16)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            v_flv_ip_list: take(17)
                .and_then(|v| v.try_into_list().ok())
                .map(|l| {
                    l.into_iter()
                        .filter_map(|x| x.try_into_string().ok())
                        .collect()
                })
                .unwrap_or_default(),
            i_is_p2p_support: take(18)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_p2p_url: take(19)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_p2p_url_suffix: take(20)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_p2p_anti_code: take(21)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            l_free_flag: take(22)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_is_hevc_support: take(23)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            v_p2p_ip_list: take(24)
                .and_then(|v| v.try_into_list().ok())
                .map(|l| {
                    l.into_iter()
                        .filter_map(|x| x.try_into_string().ok())
                        .collect()
                })
                .unwrap_or_default(),
            mp_ext_args: take(25)
                .and_then(|v| v.try_into_map().ok())
                .map(|m| {
                    m.into_iter()
                        .filter_map(|(k, v)| {
                            Some((k.try_into_string().ok()?, v.try_into_string().ok()?))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            l_timespan: take(26)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_update_time: take(27)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
        })
    }
}

// MultiStreamInfo struct from JavaScript x.MultiStreamInfo
#[derive(Default, Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct MultiStreamInfo {
    pub s_display_name: String, // tag 0
    pub i_bit_rate: i32,        // tag 1
    pub i_codec_type: i32,      // tag 2
    pub i_compatible_flag: i32, // tag 3
    pub i_hevc_bit_rate: i32,   // tag 4 (default -1)
    pub i_enable: i32,          // tag 5 (default 1)
    pub i_enable_method: i32,   // tag 6
    pub s_enable_url: String,   // tag 7
    pub s_tip_text: String,     // tag 8
    pub s_tag_text: String,     // tag 9
    pub s_tag_url: String,      // tag 10
    pub i_frame_rate: i32,      // tag 11
    pub i_sort_value: i32,      // tag 12
}

impl From<MultiStreamInfo> for TarsValue {
    fn from(info: MultiStreamInfo) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::String(info.s_display_name));
        map.insert(1, TarsValue::Int(info.i_bit_rate));
        map.insert(2, TarsValue::Int(info.i_codec_type));
        map.insert(3, TarsValue::Int(info.i_compatible_flag));
        map.insert(4, TarsValue::Int(info.i_hevc_bit_rate));
        map.insert(5, TarsValue::Int(info.i_enable));
        map.insert(6, TarsValue::Int(info.i_enable_method));
        map.insert(7, TarsValue::String(info.s_enable_url));
        map.insert(8, TarsValue::String(info.s_tip_text));
        map.insert(9, TarsValue::String(info.s_tag_text));
        map.insert(10, TarsValue::String(info.s_tag_url));
        map.insert(11, TarsValue::Int(info.i_frame_rate));
        map.insert(12, TarsValue::Int(info.i_sort_value));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for MultiStreamInfo {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(MultiStreamInfo {
            s_display_name: take(0)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_bit_rate: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_codec_type: take(2)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_compatible_flag: take(3)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_hevc_bit_rate: take(4).and_then(|v| v.try_into_i32().ok()).unwrap_or(-1),
            i_enable: take(5).and_then(|v| v.try_into_i32().ok()).unwrap_or(1),
            i_enable_method: take(6)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_enable_url: take(7)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_tip_text: take(8)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_tag_text: take(9)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_tag_url: take(10)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_frame_rate: take(11)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_sort_value: take(12)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

// BeginLiveNotice struct from JavaScript BeginLiveNotice
#[derive(Default, Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct BeginLiveNotice {
    pub l_presenter_uid: i64,                      // tag 0
    pub i_game_id: i32,                            // tag 1
    pub s_game_name: String,                       // tag 2
    pub i_random_range: i32,                       // tag 3
    pub i_stream_type: i32,                        // tag 4
    pub v_stream_info: Vec<StreamInfo>,            // tag 5
    pub v_cdn_list: Vec<String>,                   // tag 6
    pub l_live_id: i64,                            // tag 7
    pub i_pc_default_bit_rate: i32,                // tag 8
    pub i_web_default_bit_rate: i32,               // tag 9
    pub i_mobile_default_bit_rate: i32,            // tag 10
    pub l_multi_stream_flag: i64,                  // tag 11
    pub s_nick: String,                            // tag 12
    pub l_yy_id: i64,                              // tag 13
    pub l_attendee_count: i64,                     // tag 14
    pub i_codec_type: i32,                         // tag 15
    pub i_screen_type: i32,                        // tag 16
    pub v_multi_stream_info: Vec<MultiStreamInfo>, // tag 17
    pub s_live_desc: String,                       // tag 18
    pub l_live_compatible_flag: i64,               // tag 19
    pub s_avatar_url: String,                      // tag 20
    pub i_source_type: i32,                        // tag 21
    pub s_subchannel_name: String,                 // tag 22
    pub s_video_capture_url: String,               // tag 23
    pub i_start_time: i32,                         // tag 24
    pub l_channel_id: i64,                         // tag 25
    pub l_sub_channel_id: i64,                     // tag 26
    pub s_location: String,                        // tag 27
    pub i_cdn_policy_level: i32,                   // tag 28
    pub i_game_type: i32,                          // tag 29
    pub m_misc_info: FxHashMap<String, String>,    // tag 30
    pub i_short_channel: i32,                      // tag 31
    pub i_room_id: i32,                            // tag 32
    pub b_is_room_secret: i32,                     // tag 33
    pub i_hash_policy: i32,                        // tag 34
    pub l_sign_channel: i64,                       // tag 35
    pub i_mobile_wifi_default_bit_rate: i32,       // tag 36
    pub i_enable_auto_bit_rate: i32,               // tag 37
    pub i_template: i32,                           // tag 38
    pub i_replay: i32,                             // tag 39
}

impl From<BeginLiveNotice> for TarsValue {
    fn from(notice: BeginLiveNotice) -> Self {
        let mut map = FxHashMap::default();
        map.insert(0, TarsValue::Long(notice.l_presenter_uid));
        map.insert(1, TarsValue::Int(notice.i_game_id));
        map.insert(2, TarsValue::String(notice.s_game_name));
        map.insert(3, TarsValue::Int(notice.i_random_range));
        map.insert(4, TarsValue::Int(notice.i_stream_type));
        map.insert(
            5,
            TarsValue::List(
                notice
                    .v_stream_info
                    .into_iter()
                    .map(|s| Box::new(TarsValue::from(s)))
                    .collect(),
            ),
        );
        map.insert(
            6,
            TarsValue::List(
                notice
                    .v_cdn_list
                    .into_iter()
                    .map(|s| Box::new(TarsValue::String(s)))
                    .collect(),
            ),
        );
        map.insert(7, TarsValue::Long(notice.l_live_id));
        map.insert(8, TarsValue::Int(notice.i_pc_default_bit_rate));
        map.insert(9, TarsValue::Int(notice.i_web_default_bit_rate));
        map.insert(10, TarsValue::Int(notice.i_mobile_default_bit_rate));
        map.insert(11, TarsValue::Long(notice.l_multi_stream_flag));
        map.insert(12, TarsValue::String(notice.s_nick));
        map.insert(13, TarsValue::Long(notice.l_yy_id));
        map.insert(14, TarsValue::Long(notice.l_attendee_count));
        map.insert(15, TarsValue::Int(notice.i_codec_type));
        map.insert(16, TarsValue::Int(notice.i_screen_type));
        map.insert(
            17,
            TarsValue::List(
                notice
                    .v_multi_stream_info
                    .into_iter()
                    .map(|s| Box::new(TarsValue::from(s)))
                    .collect(),
            ),
        );
        map.insert(18, TarsValue::String(notice.s_live_desc));
        map.insert(19, TarsValue::Long(notice.l_live_compatible_flag));
        map.insert(20, TarsValue::String(notice.s_avatar_url));
        map.insert(21, TarsValue::Int(notice.i_source_type));
        map.insert(22, TarsValue::String(notice.s_subchannel_name));
        map.insert(23, TarsValue::String(notice.s_video_capture_url));
        map.insert(24, TarsValue::Int(notice.i_start_time));
        map.insert(25, TarsValue::Long(notice.l_channel_id));
        map.insert(26, TarsValue::Long(notice.l_sub_channel_id));
        map.insert(27, TarsValue::String(notice.s_location));
        map.insert(28, TarsValue::Int(notice.i_cdn_policy_level));
        map.insert(29, TarsValue::Int(notice.i_game_type));
        map.insert(
            30,
            TarsValue::Map(
                notice
                    .m_misc_info
                    .into_iter()
                    .map(|(k, v)| (TarsValue::String(k), TarsValue::String(v)))
                    .collect(),
            ),
        );
        map.insert(31, TarsValue::Int(notice.i_short_channel));
        map.insert(32, TarsValue::Int(notice.i_room_id));
        map.insert(33, TarsValue::Int(notice.b_is_room_secret));
        map.insert(34, TarsValue::Int(notice.i_hash_policy));
        map.insert(35, TarsValue::Long(notice.l_sign_channel));
        map.insert(36, TarsValue::Int(notice.i_mobile_wifi_default_bit_rate));
        map.insert(37, TarsValue::Int(notice.i_enable_auto_bit_rate));
        map.insert(38, TarsValue::Int(notice.i_template));
        map.insert(39, TarsValue::Int(notice.i_replay));
        TarsValue::Struct(map)
    }
}

impl TryFrom<TarsValue> for BeginLiveNotice {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        let mut map = value.try_into_struct()?;
        let mut take = |tag: u8| map.remove(&tag);

        Ok(BeginLiveNotice {
            l_presenter_uid: take(0)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_game_id: take(1)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_game_name: take(2)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_random_range: take(3)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_stream_type: take(4)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            v_stream_info: take(5)
                .and_then(|v| v.try_into_list().ok())
                .map(|l| {
                    l.into_iter()
                        .filter_map(|x| StreamInfo::try_from(*x).ok())
                        .collect()
                })
                .unwrap_or_default(),
            v_cdn_list: take(6)
                .and_then(|v| v.try_into_list().ok())
                .map(|l| {
                    l.into_iter()
                        .filter_map(|x| x.try_into_string().ok())
                        .collect()
                })
                .unwrap_or_default(),
            l_live_id: take(7)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_pc_default_bit_rate: take(8)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_web_default_bit_rate: take(9)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_mobile_default_bit_rate: take(10)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            l_multi_stream_flag: take(11)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_nick: take(12)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            l_yy_id: take(13)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_attendee_count: take(14)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_codec_type: take(15)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_screen_type: take(16)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            v_multi_stream_info: take(17)
                .and_then(|v| v.try_into_list().ok())
                .map(|l| {
                    l.into_iter()
                        .filter_map(|x| MultiStreamInfo::try_from(*x).ok())
                        .collect()
                })
                .unwrap_or_default(),
            s_live_desc: take(18)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            l_live_compatible_flag: take(19)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_avatar_url: take(20)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_source_type: take(21)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            s_subchannel_name: take(22)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            s_video_capture_url: take(23)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_start_time: take(24)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            l_channel_id: take(25)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            l_sub_channel_id: take(26)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            s_location: take(27)
                .and_then(|v| v.try_into_string().ok())
                .unwrap_or_default(),
            i_cdn_policy_level: take(28)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_game_type: take(29)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            m_misc_info: take(30)
                .and_then(|v| v.try_into_map().ok())
                .map(|m| {
                    m.into_iter()
                        .filter_map(|(k, v)| {
                            Some((k.try_into_string().ok()?, v.try_into_string().ok()?))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            i_short_channel: take(31)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_room_id: take(32)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            b_is_room_secret: take(33)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_hash_policy: take(34)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            l_sign_channel: take(35)
                .and_then(|v| v.try_into_i64().ok())
                .unwrap_or_default(),
            i_mobile_wifi_default_bit_rate: take(36)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_enable_auto_bit_rate: take(37)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_template: take(38)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
            i_replay: take(39)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_info_compatibility() {
        let info = StreamInfo {
            s_cdn_type: "aliyun".into(),
            i_is_master: 1,
            l_channel_id: 123,
            l_sub_channel_id: 456,
            v_flv_ip_list: vec!["1.1.1.1".into(), "2.2.2.2".into()],
            mp_ext_args: {
                let mut m = FxHashMap::default();
                m.insert("key".into(), "val".into());
                m
            },
            ..Default::default()
        };

        let tars_val = TarsValue::from(info.clone());
        let decoded = StreamInfo::try_from(tars_val).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn test_multi_stream_info_compatibility() {
        let info = MultiStreamInfo {
            s_display_name: "1080P".into(),
            i_bit_rate: 4000,
            i_hevc_bit_rate: 3000,
            ..Default::default()
        };

        let tars_val = TarsValue::from(info.clone());
        let decoded = MultiStreamInfo::try_from(tars_val).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn test_begin_live_notice_compatibility() {
        let notice = BeginLiveNotice {
            l_presenter_uid: 123,
            s_nick: "tester".into(),
            v_stream_info: vec![StreamInfo::default()],
            v_multi_stream_info: vec![MultiStreamInfo::default()],
            ..Default::default()
        };

        let tars_val = TarsValue::from(notice.clone());
        let decoded = BeginLiveNotice::try_from(tars_val).unwrap();
        assert_eq!(notice, decoded);
    }
}
