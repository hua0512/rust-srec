//! Stream info structures for Huya live streaming

use rustc_hash::FxHashMap;
use tars_codec::{error::TarsError, types::TarsValue};

// StreamInfo struct from JavaScript x.StreamInfo
#[derive(Default, Debug, Clone)]
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

impl TryFrom<TarsValue> for StreamInfo {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
            let take_optional_string = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> String {
                map.remove(&tag)
                    .and_then(|v| v.try_into_string().ok())
                    .unwrap_or_default()
            };
            let take_optional_i32 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i32 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i32().ok())
                    .unwrap_or_default()
            };
            let take_optional_i64 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i64 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i64().ok())
                    .unwrap_or_default()
            };

            let s_cdn_type = take_optional_string(&mut map, 0);
            let i_is_master = take_optional_i32(&mut map, 1);
            let l_channel_id = take_optional_i64(&mut map, 2);
            let l_sub_channel_id = take_optional_i64(&mut map, 3);
            let l_presenter_uid = take_optional_i64(&mut map, 4);
            let s_stream_name = take_optional_string(&mut map, 5);
            let s_flv_url = take_optional_string(&mut map, 6);
            let s_flv_url_suffix = take_optional_string(&mut map, 7);
            let s_flv_anti_code = take_optional_string(&mut map, 8);
            let s_hls_url = take_optional_string(&mut map, 9);
            let s_hls_url_suffix = take_optional_string(&mut map, 10);
            let s_hls_anti_code = take_optional_string(&mut map, 11);
            let i_line_index = take_optional_i32(&mut map, 12);
            let i_is_multi_stream = take_optional_i32(&mut map, 13);
            let i_pc_priority_rate = take_optional_i32(&mut map, 14);
            let i_web_priority_rate = take_optional_i32(&mut map, 15);
            let i_mobile_priority_rate = take_optional_i32(&mut map, 16);

            let v_flv_ip_list = map
                .remove(&17)
                .and_then(|v| {
                    if let TarsValue::List(list) = v {
                        Some(
                            list.into_iter()
                                .filter_map(|item| (*item).try_into_string().ok())
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let i_is_p2p_support = take_optional_i32(&mut map, 18);
            let s_p2p_url = take_optional_string(&mut map, 19);
            let s_p2p_url_suffix = take_optional_string(&mut map, 20);
            let s_p2p_anti_code = take_optional_string(&mut map, 21);
            let l_free_flag = take_optional_i64(&mut map, 22);
            let i_is_hevc_support = take_optional_i32(&mut map, 23);

            let v_p2p_ip_list = map
                .remove(&24)
                .and_then(|v| {
                    if let TarsValue::List(list) = v {
                        Some(
                            list.into_iter()
                                .filter_map(|item| (*item).try_into_string().ok())
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let mp_ext_args = map
                .remove(&25)
                .and_then(|v| {
                    if let TarsValue::Map(map_val) = v {
                        let mut result = FxHashMap::default();
                        for (k, v) in map_val {
                            if let (Ok(key), Ok(val)) = (k.try_into_string(), v.try_into_string()) {
                                result.insert(key, val);
                            }
                        }
                        Some(result)
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let l_timespan = take_optional_i64(&mut map, 26);
            let l_update_time = take_optional_i64(&mut map, 27);

            Ok(StreamInfo {
                s_cdn_type,
                i_is_master,
                l_channel_id,
                l_sub_channel_id,
                l_presenter_uid,
                s_stream_name,
                s_flv_url,
                s_flv_url_suffix,
                s_flv_anti_code,
                s_hls_url,
                s_hls_url_suffix,
                s_hls_anti_code,
                i_line_index,
                i_is_multi_stream,
                i_pc_priority_rate,
                i_web_priority_rate,
                i_mobile_priority_rate,
                v_flv_ip_list,
                i_is_p2p_support,
                s_p2p_url,
                s_p2p_url_suffix,
                s_p2p_anti_code,
                l_free_flag,
                i_is_hevc_support,
                v_p2p_ip_list,
                mp_ext_args,
                l_timespan,
                l_update_time,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}

// MultiStreamInfo struct from JavaScript x.MultiStreamInfo
#[derive(Default, Debug, Clone)]
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

impl TryFrom<TarsValue> for MultiStreamInfo {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
            let take_optional_string = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> String {
                map.remove(&tag)
                    .and_then(|v| v.try_into_string().ok())
                    .unwrap_or_default()
            };
            let take_optional_i32 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i32 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i32().ok())
                    .unwrap_or_default()
            };

            let s_display_name = take_optional_string(&mut map, 0);
            let i_bit_rate = take_optional_i32(&mut map, 1);
            let i_codec_type = take_optional_i32(&mut map, 2);
            let i_compatible_flag = take_optional_i32(&mut map, 3);
            let i_hevc_bit_rate = map
                .remove(&4)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or(-1);
            let i_enable = map
                .remove(&5)
                .and_then(|v| v.try_into_i32().ok())
                .unwrap_or(1);
            let i_enable_method = take_optional_i32(&mut map, 6);
            let s_enable_url = take_optional_string(&mut map, 7);
            let s_tip_text = take_optional_string(&mut map, 8);
            let s_tag_text = take_optional_string(&mut map, 9);
            let s_tag_url = take_optional_string(&mut map, 10);
            let i_frame_rate = take_optional_i32(&mut map, 11);
            let i_sort_value = take_optional_i32(&mut map, 12);

            Ok(MultiStreamInfo {
                s_display_name,
                i_bit_rate,
                i_codec_type,
                i_compatible_flag,
                i_hevc_bit_rate,
                i_enable,
                i_enable_method,
                s_enable_url,
                s_tip_text,
                s_tag_text,
                s_tag_url,
                i_frame_rate,
                i_sort_value,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}

// BeginLiveNotice struct from JavaScript BeginLiveNotice
#[derive(Default, Debug, Clone)]
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

impl TryFrom<TarsValue> for BeginLiveNotice {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        if let TarsValue::Struct(mut map) = value {
            let take_optional_string = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> String {
                map.remove(&tag)
                    .and_then(|v| v.try_into_string().ok())
                    .unwrap_or_default()
            };
            let take_optional_i32 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i32 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i32().ok())
                    .unwrap_or_default()
            };
            let take_optional_i64 = |map: &mut FxHashMap<u8, TarsValue>, tag: u8| -> i64 {
                map.remove(&tag)
                    .and_then(|v| v.try_into_i64().ok())
                    .unwrap_or_default()
            };

            let l_presenter_uid = take_optional_i64(&mut map, 0);
            let i_game_id = take_optional_i32(&mut map, 1);
            let s_game_name = take_optional_string(&mut map, 2);
            let i_random_range = take_optional_i32(&mut map, 3);
            let i_stream_type = take_optional_i32(&mut map, 4);

            let v_stream_info = map
                .remove(&5)
                .and_then(|v| {
                    if let TarsValue::List(list) = v {
                        Some(
                            list.into_iter()
                                .filter_map(|item| StreamInfo::try_from(*item).ok())
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let v_cdn_list = map
                .remove(&6)
                .and_then(|v| {
                    if let TarsValue::List(list) = v {
                        Some(
                            list.into_iter()
                                .filter_map(|item| (*item).try_into_string().ok())
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let l_live_id = take_optional_i64(&mut map, 7);
            let i_pc_default_bit_rate = take_optional_i32(&mut map, 8);
            let i_web_default_bit_rate = take_optional_i32(&mut map, 9);
            let i_mobile_default_bit_rate = take_optional_i32(&mut map, 10);
            let l_multi_stream_flag = take_optional_i64(&mut map, 11);
            let s_nick = take_optional_string(&mut map, 12);
            let l_yy_id = take_optional_i64(&mut map, 13);
            let l_attendee_count = take_optional_i64(&mut map, 14);
            let i_codec_type = take_optional_i32(&mut map, 15);
            let i_screen_type = take_optional_i32(&mut map, 16);

            let v_multi_stream_info = map
                .remove(&17)
                .and_then(|v| {
                    if let TarsValue::List(list) = v {
                        Some(
                            list.into_iter()
                                .filter_map(|item| MultiStreamInfo::try_from(*item).ok())
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let s_live_desc = take_optional_string(&mut map, 18);
            let l_live_compatible_flag = take_optional_i64(&mut map, 19);
            let s_avatar_url = take_optional_string(&mut map, 20);
            let i_source_type = take_optional_i32(&mut map, 21);
            let s_subchannel_name = take_optional_string(&mut map, 22);
            let s_video_capture_url = take_optional_string(&mut map, 23);
            let i_start_time = take_optional_i32(&mut map, 24);
            let l_channel_id = take_optional_i64(&mut map, 25);
            let l_sub_channel_id = take_optional_i64(&mut map, 26);
            let s_location = take_optional_string(&mut map, 27);
            let i_cdn_policy_level = take_optional_i32(&mut map, 28);
            let i_game_type = take_optional_i32(&mut map, 29);

            let m_misc_info = map
                .remove(&30)
                .and_then(|v| {
                    if let TarsValue::Map(map_val) = v {
                        let mut result = FxHashMap::default();
                        for (k, v) in map_val {
                            if let (Ok(key), Ok(val)) = (k.try_into_string(), v.try_into_string()) {
                                result.insert(key, val);
                            }
                        }
                        Some(result)
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let i_short_channel = take_optional_i32(&mut map, 31);
            let i_room_id = take_optional_i32(&mut map, 32);
            let b_is_room_secret = take_optional_i32(&mut map, 33);
            let i_hash_policy = take_optional_i32(&mut map, 34);
            let l_sign_channel = take_optional_i64(&mut map, 35);
            let i_mobile_wifi_default_bit_rate = take_optional_i32(&mut map, 36);
            let i_enable_auto_bit_rate = take_optional_i32(&mut map, 37);
            let i_template = take_optional_i32(&mut map, 38);
            let i_replay = take_optional_i32(&mut map, 39);

            Ok(BeginLiveNotice {
                l_presenter_uid,
                i_game_id,
                s_game_name,
                i_random_range,
                i_stream_type,
                v_stream_info,
                v_cdn_list,
                l_live_id,
                i_pc_default_bit_rate,
                i_web_default_bit_rate,
                i_mobile_default_bit_rate,
                l_multi_stream_flag,
                s_nick,
                l_yy_id,
                l_attendee_count,
                i_codec_type,
                i_screen_type,
                v_multi_stream_info,
                s_live_desc,
                l_live_compatible_flag,
                s_avatar_url,
                i_source_type,
                s_subchannel_name,
                s_video_capture_url,
                i_start_time,
                l_channel_id,
                l_sub_channel_id,
                s_location,
                i_cdn_policy_level,
                i_game_type,
                m_misc_info,
                i_short_channel,
                i_room_id,
                b_is_room_secret,
                i_hash_policy,
                l_sign_channel,
                i_mobile_wifi_default_bit_rate,
                i_enable_auto_bit_rate,
                i_template,
                i_replay,
            })
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Struct",
                actual: "Other",
            })
        }
    }
}
