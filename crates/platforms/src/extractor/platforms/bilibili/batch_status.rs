use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use super::builder::{Bilibili, URL_REGEX};
use crate::extractor::batch_status::{BatchPresence, BatchStatusProbe};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::utils::capture_group_1;

/// Bilibili's batch status probe, registered with the Bilibili platform.
pub(crate) static BATCH_PROBE: BilibiliStatusProbe = BilibiliStatusProbe {
    endpoint: Cow::Borrowed(ROOM_BASE_INFO_URL),
};

const ROOM_BASE_INFO_URL: &str =
    "https://api.live.bilibili.com/xlive/web-room/v1/index/getRoomBaseInfo";

/// The endpoint rejects more room IDs than this with code 1200002 ("too many ids").
const MAX_ROOM_IDS: usize = 100;

/// Live status of Bilibili rooms from one `getRoomBaseInfo` request. Keys are room IDs as written
/// in the streamer URL, which may be a room's short ID.
pub(crate) struct BilibiliStatusProbe {
    endpoint: Cow<'static, str>,
}

#[derive(Deserialize)]
struct RoomBaseInfoResponse {
    code: i32,
    data: Option<RoomBaseInfoData>,
}

#[derive(Deserialize)]
struct RoomBaseInfoData {
    /// Keyed by long room ID. Rooms that do not exist are left out.
    #[serde(default)]
    by_room_ids: HashMap<String, RoomBaseInfo>,
}

#[derive(Deserialize)]
struct RoomBaseInfo {
    room_id: u64,
    /// The room's short ID, or 0 when it has none.
    #[serde(default)]
    short_id: u64,
    /// 0 offline, 1 live, 2 replaying recorded videos.
    live_status: u8,
}

#[async_trait]
impl BatchStatusProbe for BilibiliStatusProbe {
    fn max_keys(&self) -> usize {
        MAX_ROOM_IDS
    }

    /// The room ID in canonical decimal form, so the key compares equal to the IDs in the
    /// response. Only IDs in `1..=i64::MAX` qualify: the endpoint rejects the whole request
    /// (code -400) when any ID is 0 or larger.
    fn key_for_url(&self, url: &str) -> Option<String> {
        let room_id: i64 = capture_group_1(&URL_REGEX, url)?.parse().ok()?;
        (room_id > 0).then(|| room_id.to_string())
    }

    async fn probe(
        &self,
        client: &Client,
        keys: &[String],
    ) -> Result<HashMap<String, BatchPresence>, ExtractorError> {
        let mut query = Vec::with_capacity(keys.len() + 1);
        query.push(("req_biz", "web_room_componet"));
        query.extend(keys.iter().map(|key| ("room_ids", key.as_str())));
        let response = client
            .get(self.endpoint.as_ref())
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header(reqwest::header::REFERER, Bilibili::BASE_URL)
            .query(&query)
            .send()
            .await?;
        let response: RoomBaseInfoResponse =
            ExtractorError::check_response(response)?.json().await?;
        // The request is anonymous, so no code is evidence against an account.
        if response.code != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Bilibili API returned code {}",
                response.code
            )));
        }
        let rooms = response
            .data
            .map(|data| data.by_room_ids)
            .unwrap_or_default();

        // A room answers to its long ID and its short ID. Long IDs are inserted last so that a
        // requested long ID matches its own room even if it equals another room's short ID.
        let mut by_id = HashMap::with_capacity(rooms.len() * 2);
        for room in rooms.values().filter(|room| room.short_id != 0) {
            by_id.insert(room.short_id, presence(room.live_status));
        }
        for room in rooms.values() {
            by_id.insert(room.room_id, presence(room.live_status));
        }
        Ok(keys
            .iter()
            .filter_map(|key| {
                let presence = by_id.get(&key.parse::<u64>().ok()?)?;
                Some((key.clone(), *presence))
            })
            .collect())
    }
}

/// Only status 1 is live, as in the extractor; a room replaying recorded videos (2) is offline.
fn presence(live_status: u8) -> BatchPresence {
    if live_status == 1 {
        BatchPresence::Live
    } else {
        BatchPresence::Offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;
    use crate::extractor::test_support::serve_once;

    async fn probe_against(
        body: &'static str,
        keys: &[&str],
    ) -> Result<HashMap<String, BatchPresence>, ExtractorError> {
        let probe = BilibiliStatusProbe {
            endpoint: Cow::Owned(serve_once("200 OK", body).await),
        };
        let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            probe.probe(&default_client(), &keys),
        )
        .await
        .expect("local Bilibili request timed out")
    }

    /// Short IDs reach the long-ID entries that answer for them, each status maps to its
    /// presence, and a room the response leaves out stays unknown.
    #[tokio::test]
    async fn rooms_are_matched_by_long_and_short_id() {
        let body = r#"{"code":0,"message":"OK","ttl":1,"data":{"by_uids":{},"by_room_ids":{
            "7734200":{"room_id":7734200,"short_id":6,"live_status":1},
            "5440":{"room_id":5440,"short_id":1,"live_status":2},
            "21452505":{"room_id":21452505,"short_id":0,"live_status":0}
        }}}"#;

        let result = probe_against(body, &["6", "1", "21452505", "999999999999"])
            .await
            .unwrap();

        assert_eq!(
            result,
            HashMap::from([
                ("6".to_owned(), BatchPresence::Live),
                ("1".to_owned(), BatchPresence::Offline),
                ("21452505".to_owned(), BatchPresence::Offline),
            ])
        );
    }

    #[tokio::test]
    async fn nonzero_code_is_an_error() {
        let body = r#"{"code":1200002,"message":"too many ids","ttl":1,"data":{"by_uids":{},"by_room_ids":{}}}"#;

        let error = probe_against(body, &["6"]).await.unwrap_err();

        assert!(
            matches!(&error, ExtractorError::ValidationError(message) if message.contains("1200002")),
            "{error:?}"
        );
    }
}
