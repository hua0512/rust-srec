use serde::{Deserialize, Serialize};

use crate::media::PlatformMetadata;

#[derive(Serialize, Deserialize)]
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

/// The broadcaster's uid in metadata a Bigo check observed, or `None` for
/// another platform's metadata, another version, or a value that is not a
/// valid uid.
pub fn metadata_uid(metadata: &PlatformMetadata) -> Option<u32> {
    if metadata.platform != "bigo" || metadata.schema_version != 1 {
        return None;
    }
    BigoMetadataV1::deserialize(&metadata.data)
        .ok()?
        .uid
        .parse()
        .ok()
        .filter(|&uid| uid != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uid_reads_back_from_the_metadata_a_check_writes() {
        let written = metadata(123_456).unwrap();
        assert_eq!(metadata_uid(&written), Some(123_456));
        let other = PlatformMetadata {
            platform: "other".to_owned(),
            ..written
        };
        assert_eq!(metadata_uid(&other), None);
    }
}
