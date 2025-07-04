#![allow(dead_code, unused_variables)]
use std::borrow::Cow;

use serde::Deserialize;

// Custom deserializer to handle data being either an object or an empty array
fn deserialize_mp_data<'a, D>(deserializer: D) -> Result<Option<MpData<'a>>, D::Error>
where
    D: serde::Deserializer<'a>,
{
    use serde::de::{self, Visitor};

    struct MpDataVisitor;

    impl<'a> Visitor<'a> for MpDataVisitor {
        type Value = Option<MpData<'a>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an object or an array")
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'a>,
        {
            // If it's a map/object, deserialize as MpData
            let mp_data = MpData::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(Some(mp_data))
        }

        fn visit_seq<A>(self, _seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'a>,
        {
            // If it's a sequence/array (error case), return None
            Ok(None)
        }
    }

    deserializer.deserialize_any(MpDataVisitor)
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MpApiResponse<'a> {
    pub status: i32,
    #[serde(borrow)]
    pub message: &'a str,
    #[serde(deserialize_with = "deserialize_mp_data")]
    pub data: Option<MpData<'a>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MpData<'a> {
    #[serde(borrow)]
    pub real_live_status: Option<&'a str>,
    #[serde(borrow)]
    pub live_status: Option<&'a str>,
    pub profile_info: Option<ProfileInfo<'a>>,
    pub live_data: Option<LiveData<'a>>,
    pub stream: Option<StreamData<'a>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo<'a> {
    pub uid: i64,
    #[serde(borrow)]
    pub nick: Cow<'a, str>,
    #[serde(borrow)]
    pub avatar180: Cow<'a, str>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LiveData<'a> {
    #[serde(borrow)]
    pub introduction: Cow<'a, str>,
    #[serde(borrow)]
    pub screenshot: Cow<'a, str>,
    pub bit_rate: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StreamData<'a> {
    #[serde(default, rename = "baseSteamInfoList")]
    #[serde(borrow)]
    pub base_steam_info_list: Vec<StreamInfoItem<'a>>,
    #[serde(default)]
    #[serde(borrow)]
    pub bit_rate_info: Vec<BitrateInfo<'a>>,
    // Fallback fields for older API versions
    pub flv: Option<RateContainer<'a>>,
    pub hls: Option<RateContainer<'a>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RateContainer<'a> {
    #[serde(default)]
    #[serde(borrow)]
    pub rate_array: Vec<BitrateInfo<'a>>,
}

// This struct is for the TT_ROOM_DATA variable
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RoomData<'a> {
    #[serde(borrow)]
    pub state: &'a str,
    #[serde(borrow)]
    pub introduction: Cow<'a, str>,
}

// This struct is for the TT_PROFILE_INFO variable
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WebProfileInfo<'a> {
    pub lp: i64, // presenter uid
    #[serde(borrow)]
    pub nick: Cow<'a, str>,
    #[serde(borrow)]
    pub avatar: Cow<'a, str>,
}

// This struct is for the main stream data object
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WebStreamResponse<'a> {
    #[serde(default)]
    #[serde(borrow)]
    pub data: Vec<WebStreamDataContainer<'a>>,
    #[serde(default, rename = "vMultiStreamInfo")]
    #[serde(borrow)]
    pub v_multi_stream_info: Vec<BitrateInfo<'a>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WebStreamDataContainer<'a> {
    #[serde(borrow)]
    pub game_live_info: GameLiveInfo<'a>,
    #[serde(default, rename = "gameStreamInfoList")]
    #[serde(borrow)]
    pub game_stream_info_list: Vec<StreamInfoItem<'a>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GameLiveInfo<'a> {
    pub uid: i64,
    #[serde(borrow)]
    pub room_name: Cow<'a, str>,
    #[serde(borrow)]
    pub nick: Cow<'a, str>,
    #[serde(borrow)]
    pub screenshot: Cow<'a, str>,
    pub bit_rate: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfoItem<'a> {
    #[serde(borrow)]
    pub s_stream_name: &'a str,
    #[serde(borrow)]
    pub s_flv_url: Cow<'a, str>,
    #[serde(borrow)]
    pub s_flv_url_suffix: &'a str,
    #[serde(borrow)]
    pub s_flv_anti_code: &'a str,
    #[serde(borrow)]
    pub s_hls_url: Cow<'a, str>,
    #[serde(borrow)]
    pub s_hls_url_suffix: &'a str,
    #[serde(borrow)]
    pub s_hls_anti_code: &'a str,
    #[serde(borrow)]
    pub s_cdn_type: &'a str,
    pub i_web_priority_rate: i32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BitrateInfo<'a> {
    #[serde(borrow)]
    pub s_display_name: Cow<'a, str>,
    pub i_bit_rate: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_mp_api_response() {
        let json = include_str!("../../tests/test_data/huya/mp_api_response.json");
        let result: Result<MpApiResponse<'_>, _> = serde_json::from_str(json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deserialize_room_data() {
        let json = include_str!("../../tests/test_data/huya/web_room_data.json");
        let result: Result<RoomData<'_>, _> = serde_json::from_str(json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deserialize_web_profile_info() {
        let json = include_str!("../../tests/test_data/huya/web_profile_info.json");
        let result: Result<WebProfileInfo<'_>, _> = serde_json::from_str(json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deserialize_web_stream_response() {
        let json = include_str!("../../tests/test_data/huya/web_stream_data.json");
        let result: Result<WebStreamResponse<'_>, _> = serde_json::from_str(json);
        assert!(result.is_ok());
    }
}
