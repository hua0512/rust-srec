//! Persisted identity observations used to qualify platform batch checks.

use platforms_parser::media::PlatformMetadata;
use serde::{Deserialize, Serialize};

const IDENTITY_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
const VERIFICATION_WRITE_INTERVAL_MS: i64 = 60 * 60 * 1_000;

/// The platform owns the payload; the backend owns its verification time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPlatformMetadata {
    #[serde(flatten)]
    pub metadata: PlatformMetadata,
    /// Start of the individual check that verified this identity, in UTC epoch milliseconds.
    pub verified_at_ms: i64,
}

impl StoredPlatformMetadata {
    /// Returns a fresh supported observation for the actual extractor platform.
    /// Malformed documents and future versions remain stored but cannot suppress extraction.
    pub fn fresh(raw: Option<&str>, platform: &str, now_ms: i64) -> Option<PlatformMetadata> {
        let raw = raw?;
        let stored: Self = match serde_json::from_str(raw) {
            Ok(stored) => stored,
            Err(error) => {
                tracing::debug!(%error, "Stored platform metadata is not usable");
                return None;
            }
        };
        let age = now_ms.checked_sub(stored.verified_at_ms)?;
        (stored.verified_at_ms > 0
            && (0..IDENTITY_LIFETIME_MS).contains(&age)
            && stored.metadata.platform == platform.to_ascii_lowercase()
            && stored.metadata.schema_version == 1
            && stored.metadata.data.is_object())
        .then_some(stored.metadata)
    }

    pub(crate) fn needs_write(&self, previous: Option<&str>) -> bool {
        let Some(document) =
            previous.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        else {
            return true;
        };
        // Future versions may use a different envelope. Read their discriminator
        // before attempting to deserialize the fields this binary understands.
        if document.get("platform").and_then(serde_json::Value::as_str)
            == Some(self.metadata.platform.as_str())
            && document
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|version| version > u64::from(self.metadata.schema_version))
        {
            return false;
        }
        let Ok(previous) = serde_json::from_value::<Self>(document) else {
            return true;
        };
        previous.metadata != self.metadata
            || previous.verified_at_ms <= 0
            || previous.verified_at_ms > self.verified_at_ms
            || self.verified_at_ms.saturating_sub(previous.verified_at_ms)
                >= VERIFICATION_WRITE_INTERVAL_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(verified_at_ms: i64) -> StoredPlatformMetadata {
        StoredPlatformMetadata {
            metadata: PlatformMetadata {
                platform: "sample".into(),
                schema_version: 1,
                data: serde_json::json!({"source_key":"channel", "channel_key":"stable-key"}),
            },
            verified_at_ms,
        }
    }

    #[test]
    fn only_fresh_matching_supported_documents_can_skip_extraction() {
        let verified = 1_800_000_000_000;
        let raw = serde_json::to_string(&observation(verified)).unwrap();
        assert!(StoredPlatformMetadata::fresh(Some(&raw), "Sample", verified).is_some());
        assert!(StoredPlatformMetadata::fresh(Some(&raw), "sample", verified - 1).is_none());
        assert!(
            StoredPlatformMetadata::fresh(Some(&raw), "sample", verified + IDENTITY_LIFETIME_MS)
                .is_none()
        );
        assert!(StoredPlatformMetadata::fresh(Some(&raw), "other", verified).is_none());
        for raw in ["null", "{", "{}"] {
            assert!(StoredPlatformMetadata::fresh(Some(raw), "sample", verified).is_none());
        }
        let mut future = observation(verified);
        future.metadata.schema_version = 2;
        let raw = serde_json::to_string(&future).unwrap();
        assert!(StoredPlatformMetadata::fresh(Some(&raw), "sample", verified).is_none());
        assert!(!observation(verified + IDENTITY_LIFETIME_MS).needs_write(Some(&raw)));
        assert!(!observation(verified).needs_write(Some(
            r#"{"platform":"sample","schema_version":2,"new_envelope":true}"#
        )));
    }

    #[test]
    fn unchanged_identity_is_written_hourly_and_changes_are_immediate() {
        let verified = 1_800_000_000_000;
        let raw = serde_json::to_string(&observation(verified)).unwrap();
        assert!(!observation(verified + 1).needs_write(Some(&raw)));
        assert!(observation(verified + VERIFICATION_WRITE_INTERVAL_MS).needs_write(Some(&raw)));
        let mut changed = observation(verified + 1);
        changed.metadata.data["channel_key"] = "different-key".into();
        assert!(changed.needs_write(Some(&raw)));
        assert!(changed.needs_write(Some("invalid")));
    }
}
