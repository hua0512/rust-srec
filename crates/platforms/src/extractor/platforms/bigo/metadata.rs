use serde::Serialize;

use crate::media::PlatformMetadata;

#[derive(Serialize)]
pub(super) struct BigoMetadataV1 {
    /// The broadcaster's numeric account id, in decimal.
    pub uid: String,
}

/// The identity a check observed, or `None` when `uid` is not a valid BIGO
/// account id: positive and within 32 bits, the width BIGO's app protocol
/// gives it.
pub(super) fn metadata(uid: u64) -> Option<PlatformMetadata> {
    let uid = u32::try_from(uid).ok().filter(|&uid| uid != 0)?;
    let identity = BigoMetadataV1 {
        uid: uid.to_string(),
    };
    Some(PlatformMetadata {
        platform: "bigo".to_owned(),
        schema_version: 1,
        data: serde_json::json!(identity),
    })
}
