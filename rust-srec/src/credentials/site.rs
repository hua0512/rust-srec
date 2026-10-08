//! Sites of Streamlink accounts.
//!
//! The Streamlink platform hosts every site without a built-in extractor, so
//! an account there names the sites it is for. A site is a host name that
//! covers itself and every subdomain: `youtube.com` covers `www.youtube.com`
//! and `m.youtube.com`, but not `notyoutube.com`.

use serde::Serialize;

use super::ProfileError;

/// How many sites one account may name.
pub const MAX_SITES: usize = 32;

/// A Streamlink streamer's site and the accounts that name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct StreamerSite {
    /// The host of the streamer's URL, without a leading `www.`.
    pub host: String,
    /// The most specific site of an account that covers the host. Without
    /// an account of its own, the streamer uses that account.
    pub site: Option<String>,
    /// The accounts whose sites cover the host, most specific site first.
    pub accounts: Vec<String>,
}

/// The host `url` names: lowercase ASCII, without a trailing dot, with IPv6
/// addresses in brackets. `None` when `url` has no host.
fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

/// The site `input` names, as stored: the host of a host name or URL, without
/// a leading `www.`. A domain needs at least two labels, so a site never
/// covers a whole top-level domain.
pub fn normalize_site(input: &str) -> Result<String, ProfileError> {
    let input = input.trim();
    let invalid =
        || ProfileError::InvalidSite(format!("{input:?} is not a site such as youtube.com"));
    if input.is_empty() {
        return Err(invalid());
    }
    let url = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let host = host(&url).ok_or_else(invalid)?;
    if is_ip(&host) {
        return Ok(host);
    }
    let host = match host.strip_prefix("www.") {
        Some(rest) if rest.contains('.') => rest.to_owned(),
        _ => host,
    };
    if !host.contains('.') || host.len() > 253 {
        return Err(invalid());
    }
    Ok(host)
}

/// `sites` as an account on `platform_name` stores them: normalized, sorted
/// and without duplicates. Only Streamlink accounts name sites.
pub fn normalize_sites(platform_name: &str, sites: &[String]) -> Result<Vec<String>, ProfileError> {
    if sites.is_empty() {
        return Ok(Vec::new());
    }
    if !crate::domain::is_streamlink_platform(platform_name) {
        return Err(ProfileError::InvalidSite(
            "Only Streamlink accounts name the sites they are for".to_owned(),
        ));
    }
    let mut normalized = sites
        .iter()
        .map(|site| normalize_site(site))
        .collect::<Result<Vec<_>, _>>()?;
    normalized.sort();
    normalized.dedup();
    if normalized.len() > MAX_SITES {
        return Err(ProfileError::InvalidSite(format!(
            "An account names at most {MAX_SITES} sites"
        )));
    }
    Ok(normalized)
}

/// The host of a streamer's `url` as shown beside its sites: without a
/// leading `www.`.
pub fn site_host(url: &str) -> Option<String> {
    let host = host(url)?;
    Some(match host.strip_prefix("www.") {
        Some(rest) if rest.contains('.') => rest.to_owned(),
        _ => host,
    })
}

/// The sites that would cover a streamer's `url`, most specific first: its
/// host and every parent domain with at least two labels.
pub fn covering_sites(url: &str) -> Vec<String> {
    let Some(host) = host(url) else {
        return Vec::new();
    };
    if is_ip(&host) {
        return vec![host];
    }
    let mut sites = Vec::new();
    let mut rest = host.as_str();
    while rest.contains('.') {
        sites.push(rest.to_owned());
        rest = match rest.split_once('.') {
            Some((_, parent)) => parent,
            None => break,
        };
    }
    sites
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_is_the_host_of_a_name_or_url() {
        for (input, site) in [
            ("youtube.com", "youtube.com"),
            ("  YouTube.COM ", "youtube.com"),
            ("www.youtube.com", "youtube.com"),
            ("https://www.youtube.com/@someone/live", "youtube.com"),
            ("m.youtube.com", "m.youtube.com"),
            ("kick.com.", "kick.com"),
            ("http://example.test:8080/path", "example.test"),
            ("www.co", "www.co"),
            ("例え.jp", "xn--r8jz45g.jp"),
            ("192.168.1.10", "192.168.1.10"),
            ("http://[::1]:80/", "[::1]"),
        ] {
            assert_eq!(normalize_site(input).unwrap(), site, "{input}");
        }
    }

    #[test]
    fn a_site_needs_a_domain_below_the_top_level() {
        for input in [
            "",
            "   ",
            "com",
            "www.",
            "localhost",
            "https://",
            "exa mple.com",
        ] {
            assert!(
                matches!(normalize_site(input), Err(ProfileError::InvalidSite(_))),
                "{input:?}"
            );
        }
    }

    #[test]
    fn only_streamlink_accounts_name_sites() {
        assert_eq!(
            normalize_sites(
                "streamlink",
                &[
                    "Kick.com".into(),
                    "www.youtube.com".into(),
                    "kick.com".into()
                ]
            )
            .unwrap(),
            ["kick.com", "youtube.com"]
        );
        assert!(normalize_sites("bilibili", &[]).unwrap().is_empty());
        assert!(matches!(
            normalize_sites("bilibili", &["bilibili.com".into()]),
            Err(ProfileError::InvalidSite(_))
        ));
        let many: Vec<String> = (0..=MAX_SITES).map(|n| format!("s{n}.example")).collect();
        assert!(matches!(
            normalize_sites("streamlink", &many),
            Err(ProfileError::InvalidSite(_))
        ));
    }

    #[test]
    fn a_url_is_covered_by_its_host_and_parent_domains() {
        assert_eq!(
            covering_sites("https://www.youtube.com/@someone/live"),
            ["www.youtube.com", "youtube.com"]
        );
        assert_eq!(covering_sites("https://kick.com/someone"), ["kick.com"]);
        assert_eq!(
            covering_sites("https://a.b.example.co.uk/"),
            [
                "a.b.example.co.uk",
                "b.example.co.uk",
                "example.co.uk",
                "co.uk"
            ]
        );
        assert_eq!(covering_sites("http://10.0.0.2:8080/live"), ["10.0.0.2"]);
        assert!(covering_sites("not a url").is_empty());
        assert_eq!(
            site_host("https://www.youtube.com/@someone").as_deref(),
            Some("youtube.com")
        );
    }
}
