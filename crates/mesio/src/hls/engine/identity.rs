//! Typed segment identity.
//!
//! `SegmentKey` is the canonical identity for lifecycle and scheduling. Dedup
//! across playlist refreshes, retries, and prefetch promotion all compare keys,
//! never raw URLs. See `docs/HLS_ENGINE_ARCHITECTURE.md` (Core Data Model).

use std::sync::Arc;
use url::Url;

/// Init and media at the same URI are distinct resources, so the kind is part
/// of identity. Prefetch is deliberately *not* a kind: a Twitch prefetch URL is
/// the same resource that reappears as a normal media segment on the next
/// refresh, so prefetch-ness lives on `SegmentDescriptor::source` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SegmentKind {
    Init,
    Media,
}

/// Resolved absolute byte range. `offset` is never optional here: the manifest
/// planner must infer a missing offset from the prior segment's end before a
/// key can exist. A BYTERANGE with no explicit offset and no inferable
/// predecessor is a skip, not an `offset == 0` guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ByteRangeKey {
    pub length: u64,
    pub offset: u64,
}

/// Canonical identity for a downloadable segment resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SegmentKey {
    pub kind: SegmentKind,
    /// Normalized identity URI produced by the source's [`IdentityPolicy`].
    /// This is *not* necessarily the URL that gets fetched (see
    /// `SegmentDescriptor::parsed_url`).
    pub uri: Arc<str>,
    pub byte_range: Option<ByteRangeKey>,
}

/// Produces the canonical identity URI for a resolved segment (or key) URL.
///
/// Token-bearing CDNs can rotate auth query parameters on every refresh while
/// the underlying segment is unchanged; the policy decides which parts of the
/// URL participate in identity. The default policy keeps the full resolved URL
/// so unknown sources are never under-deduplicated.
pub trait IdentityPolicy: Send + Sync {
    fn canonical_uri(&self, resolved: &Url) -> Arc<str>;
}

/// Default policy: the full resolved URL is the identity. Rotated auth params
/// fork identity under this policy — that is the documented trade-off; merging
/// genuinely distinct segments would be the worse failure.
#[derive(Debug, Default, Clone, Copy)]
pub struct FullUrlIdentity;

impl IdentityPolicy for FullUrlIdentity {
    fn canonical_uri(&self, resolved: &Url) -> Arc<str> {
        Arc::from(resolved.as_str())
    }
}

/// Token-aware policy: identity is scheme/host/path plus all query keys except
/// the configured insignificant ones (rotating tokens, signatures, expiries).
/// Remaining query pairs are sorted by key so parameter order cannot fork
/// identity.
#[derive(Debug, Clone)]
pub struct StripQueryIdentity {
    insignificant: Vec<Box<str>>,
}

impl StripQueryIdentity {
    pub fn new<I, S>(insignificant_keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Box<str>>,
    {
        Self {
            insignificant: insignificant_keys.into_iter().map(Into::into).collect(),
        }
    }

    fn is_insignificant(&self, key: &str) -> bool {
        self.insignificant.iter().any(|k| k.as_ref() == key)
    }
}

impl IdentityPolicy for StripQueryIdentity {
    fn canonical_uri(&self, resolved: &Url) -> Arc<str> {
        let mut kept: Vec<(String, String)> = resolved
            .query_pairs()
            .filter(|(k, _)| !self.is_insignificant(k))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        if kept.len() == resolved.query_pairs().count() && resolved.query().is_some() {
            // Nothing stripped: keep the URL verbatim (cheapest stable form).
            return Arc::from(resolved.as_str());
        }

        kept.sort();
        let mut canonical = resolved.clone();
        canonical.set_fragment(None);
        if kept.is_empty() {
            canonical.set_query(None);
        } else {
            let mut pairs = canonical.query_pairs_mut();
            pairs.clear();
            for (k, v) in &kept {
                pairs.append_pair(k, v);
            }
        }
        Arc::from(canonical.as_str())
    }
}

/// Runtime-selectable identity policy. Dispatch is enum-based for the same
/// reason `CryptoExecutor` is: the set is closed and `dyn` adds nothing here.
#[derive(Debug, Clone)]
pub enum SegmentIdentityPolicy {
    FullUrl(FullUrlIdentity),
    StripQuery(StripQueryIdentity),
}

impl Default for SegmentIdentityPolicy {
    fn default() -> Self {
        Self::FullUrl(FullUrlIdentity)
    }
}

impl IdentityPolicy for SegmentIdentityPolicy {
    fn canonical_uri(&self, resolved: &Url) -> Arc<str> {
        match self {
            Self::FullUrl(p) => p.canonical_uri(resolved),
            Self::StripQuery(p) => p.canonical_uri(resolved),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url")
    }

    #[test]
    fn init_and_media_at_same_uri_are_distinct_keys() {
        let uri: Arc<str> = Arc::from("https://example.com/seg.mp4");
        let init = SegmentKey {
            kind: SegmentKind::Init,
            uri: Arc::clone(&uri),
            byte_range: None,
        };
        let media = SegmentKey {
            kind: SegmentKind::Media,
            uri,
            byte_range: None,
        };
        assert_ne!(init, media);
    }

    #[test]
    fn byte_ranges_at_same_uri_are_distinct_keys() {
        let uri: Arc<str> = Arc::from("https://example.com/all.ts");
        let a = SegmentKey {
            kind: SegmentKind::Media,
            uri: Arc::clone(&uri),
            byte_range: Some(ByteRangeKey {
                length: 10,
                offset: 0,
            }),
        };
        let b = SegmentKey {
            kind: SegmentKind::Media,
            uri,
            byte_range: Some(ByteRangeKey {
                length: 10,
                offset: 10,
            }),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn full_url_policy_keeps_rotating_params_distinct() {
        let policy = FullUrlIdentity;
        let a = policy.canonical_uri(&url("https://e.com/s1.ts?token=aaa"));
        let b = policy.canonical_uri(&url("https://e.com/s1.ts?token=bbb"));
        assert_ne!(a, b);
    }

    #[test]
    fn strip_query_policy_collapses_rotated_auth_params() {
        let policy = StripQueryIdentity::new(["token", "sign", "expire"]);
        let a = policy.canonical_uri(&url("https://e.com/s1.ts?token=aaa&sign=x&v=2"));
        let b = policy.canonical_uri(&url("https://e.com/s1.ts?token=bbb&sign=y&v=2"));
        assert_eq!(a, b);
        // The significant key survives in identity.
        assert!(a.contains("v=2"), "significant key must remain: {a}");
    }

    #[test]
    fn strip_query_policy_keeps_distinct_segments_distinct() {
        let policy = StripQueryIdentity::new(["token"]);
        let a = policy.canonical_uri(&url("https://e.com/s1.ts?token=aaa"));
        let b = policy.canonical_uri(&url("https://e.com/s2.ts?token=aaa"));
        assert_ne!(a, b);
    }

    #[test]
    fn strip_query_policy_sorts_significant_params() {
        let policy = StripQueryIdentity::new(["token"]);
        let a = policy.canonical_uri(&url("https://e.com/s1.ts?b=2&a=1&token=x"));
        let b = policy.canonical_uri(&url("https://e.com/s1.ts?a=1&b=2&token=y"));
        assert_eq!(a, b);
    }
}
