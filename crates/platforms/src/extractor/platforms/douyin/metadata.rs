use serde::{Deserialize, Serialize};

use crate::media::PlatformMetadata;

#[derive(Deserialize, Serialize)]
pub(super) struct DouyinMetadataV1 {
    pub web_rid: String,
    pub user_id: String,
}

pub(super) fn canonical_user_id(value: &str) -> Option<String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = value.parse::<i64>().ok()?;
    (id > 0).then(|| id.to_string())
}

pub(super) fn metadata(web_rid: &str, user_id: &str) -> Option<PlatformMetadata> {
    let identity = DouyinMetadataV1 {
        web_rid: web_rid.to_owned(),
        user_id: canonical_user_id(user_id)?,
    };
    Some(PlatformMetadata {
        platform: "douyin".to_owned(),
        schema_version: 1,
        data: serde_json::json!(identity),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_accepts_only_positive_decimal_user_ids() {
        for invalid in ["", "0", "-1", "+1", "1,2", "1e3", "9223372036854775808"] {
            assert!(metadata("page", invalid).is_none(), "{invalid}");
        }
        for (value, expected) in [("00123", "123"), ("9007199254740993", "9007199254740993")] {
            let metadata = metadata("page", value).unwrap();
            let identity: DouyinMetadataV1 = serde_json::from_value(metadata.data).unwrap();
            assert_eq!(identity.user_id, expected);
            assert_eq!(identity.web_rid, "page");
        }
    }
}
