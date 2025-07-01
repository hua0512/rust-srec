// struct UserId {
//     #[tars(tag = 1)]
//     s_uid: String,
//     #[tars(tag = 2)]
//     l_yyuid: i64,
//     #[tars(tag = 3)]
//     i_account_type: i32,
//     #[tars(tag = 4)]
//     s_guid: String,
//     #[tars(tag = 5)]
//     s_token: String,
//     #[tars(tag = 6)]
//     s_device_id: String,
//     #[tars(tag = 7)]
//     s_app_id: String,
//     #[tars(tag = 8)]
//     s_biz: String,
//     #[tars(tag = 9)]
//     l_tid: i64,
//     #[tars(tag = 10)]
//     l_sid: i64,
//     #[tars(tag = 11)]
//     s_trace: String,
//     #[tars(tag = 12)]
//     s_ua: String,
//     #[tars(tag = 13)]
//     s_version: String,
//     #[tars(tag = 14)]
//     s_did: String,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct GetLivingInfoReq {
//     #[tars(tag = 1)]
//     t_id: UserId,
//     #[tars(tag = 2)]
//     l_top_sid: i64,
//     #[tars(tag = 3)]
//     l_sub_sid: i64,
//     #[tars(tag = 4)]
//     s_trace: String,
//     #[tars(tag = 5)]
//     s_from: String,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct GetLivingInfoRsp {
//     #[tars(tag = 1)]
//     t_notice: LiveLaunchRsp,
//     #[tars(tag = 2)]
//     e_result: i32,
//     #[tars(tag = 3)]
//     s_message: String,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct LiveLaunchRsp {
//     #[tars(tag = 1)]
//     l_sid: i64,
//     #[tars(tag = 2)]
//     l_sub_sid: i64,
//     #[tars(tag = 3)]
//     s_trace: String,
//     #[tars(tag = 4)]
//     t_live_info: LiveInfo,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct LiveInfo {
//     #[tars(tag = 0)]
//     l_yy_id: i64,
//     #[tars(tag = 1)]
//     s_nick: String,
//     #[tars(tag = 2)]
//     i_gender: i32,
//     #[tars(tag = 3)]
//     s_avatar_url: String,
//     #[tars(tag = 4)]
//     t_stream_info: List<TarsStreamInfo>,
//     #[tars(tag = 5)]
//     l_live_id: i64,
//     #[tars(tag = 6)]
//     s_room_name: String,
//     #[tars(tag = 7)]
//     s_screenshot_url: String,
//     #[tars(tag = 8)]
//     i_bit_rate: i32,
//     #[tars(tag = 9)]
//     v_bit_rate_info: List<BitRateInfo>,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct TarsStreamInfo {
//     #[tars(tag = 0)]
//     s_stream_name: String,
//     #[tars(tag = 1)]
//     s_cdn_type: String,
//     #[tars(tag = 2)]
//     i_cdn_node: CdnNode,
//     #[tars(tag = 3)]
//     i_is_master: i32,
//     #[tars(tag = 4)]
//     l_channel_id: i64,
//     #[tars(tag = 5)]
//     l_sub_channel_id: i64,
//     #[tars(tag = 6)]
//     s_url: String,
//     #[tars(tag = 7)]
//     s_flv_url: String,
//     #[tars(tag = 8)]
//     s_hls_url: String,
//     #[tars(tag = 9)]
//     s_p2p_url: String,
//     #[tars(tag = 10)]
//     s_flv_url_suffix: String,
//     #[tars(tag = 11)]
//     s_hls_url_suffix: String,
//     #[tars(tag = 12)]
//     s_flv_anti_code: String,
//     #[tars(tag = 13)]
//     s_hls_anti_code: String,
//     #[tars(tag = 14)]
//     i_web_priority_rate: i32,
//     #[tars(tag = 15)]
//     i_stream_status: i32,
//     #[tars(tag = 16)]
//     i_p2p_stream_type: i32,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct CdnNode {
//     #[tars(tag = 0)]
//     s_cdn_node_name: String,
//     #[tars(tag = 1)]
//     i_cdn_node_type: i32,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[tars(tag = 0, servant = "live")]
// struct BitRateInfo {
//     #[tars(tag = 0)]
//     s_display_name: String,
//     #[tars(tag = 1)]
//     i_bit_rate: i32,
//     #[tars(tag = 2)]
//     i_codec_type: i32,
// }