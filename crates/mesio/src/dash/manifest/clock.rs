//! Clock synchronization for dynamic presentations (ISO/IEC 23009-1 §5.8.4.11
//! `UTCTiming` and the §5.8.5.7 schemes; DASH-IF IOP v4.3 §4.7).
//!
//! Segment availability is computed against the server's clock. The engine
//! measures an offset once per MPD fetch, in this order of preference: the
//! MPD's `UTCTiming` sources in document order, then the MPD response's HTTP
//! `Date` header, then no correction. Everything here is pure; fetching is
//! the caller's job.

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use parking_lot::Mutex;
use url::Url;

use super::error::ManifestError;
use super::xs;

/// A supported `UTCTiming` source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UtcTiming {
    /// `urn:mpeg:dash:utc:http-iso:2014`: GET returns an ISO 8601 date-time.
    HttpIso { urls: Vec<Url> },
    /// `urn:mpeg:dash:utc:http-xsdate:2014`: GET returns an `xs:dateTime`.
    HttpXsDate { urls: Vec<Url> },
    /// `urn:mpeg:dash:utc:http-head:2014`: HEAD, read the `Date` header.
    HttpHead { urls: Vec<Url> },
    /// `urn:mpeg:dash:utc:direct:2014`: the time is the descriptor's value,
    /// valid at the moment the MPD was fetched.
    Direct { time: DateTime<Utc> },
}

impl UtcTiming {
    /// Interprets a `UTCTiming` descriptor; `None` for schemes this engine does
    /// not implement (NTP variants, unknown URIs) or a malformed value, so the
    /// next descriptor can be tried.
    ///
    /// `@value` URLs are resolved against the MPD document URL. Both the
    /// `:2014` scheme URIs and the earlier `:2012` ones are accepted.
    pub(crate) fn from_descriptor(
        scheme_id_uri: &str,
        value: Option<&str>,
        document_url: &Url,
    ) -> Option<Self> {
        let scheme = scheme_id_uri
            .strip_suffix(":2014")
            .or_else(|| scheme_id_uri.strip_suffix(":2012"))?
            .strip_prefix("urn:mpeg:dash:utc:")?;
        let value = value?;
        let urls = || -> Option<Vec<Url>> {
            let urls: Vec<Url> = value
                .split_whitespace()
                .filter_map(|url| document_url.join(url).ok())
                .filter(|url| matches!(url.scheme(), "http" | "https"))
                .collect();
            (!urls.is_empty()).then_some(urls)
        };
        match scheme {
            "http-iso" => Some(Self::HttpIso { urls: urls()? }),
            "http-xsdate" => Some(Self::HttpXsDate { urls: urls()? }),
            "http-head" => Some(Self::HttpHead { urls: urls()? }),
            "direct" => xs::parse_date_time(value)
                .ok()
                .map(|time| Self::Direct { time }),
            _ => None,
        }
    }

    /// Reads the server time from an `http-iso` or `http-xsdate` response
    /// body. Both carry the extended ISO 8601 / `xs:dateTime` form; a
    /// timezone-less value is UTC.
    pub fn parse_body(body: &[u8]) -> Result<DateTime<Utc>, ManifestError> {
        const CONTEXT: &str = "UTCTiming response";
        let text = std::str::from_utf8(body)
            .map_err(|_| ManifestError::InvalidTime { context: CONTEXT })?;
        xs::parse_date_time(text).map_err(|_| ManifestError::InvalidTime { context: CONTEXT })
    }
}

/// Parses an HTTP `Date` header (RFC 9110 §5.6.7): IMF-fixdate, plus the
/// obsolete RFC 850 and asctime forms recipients must accept.
///
/// The header has one-second resolution, which bounds the accuracy of an
/// offset derived from it.
pub fn parse_http_date(value: &str) -> Result<DateTime<Utc>, ManifestError> {
    let value = value.trim();
    let invalid = || ManifestError::InvalidTime {
        context: "HTTP Date header",
    };
    let fixdate = value
        .strip_suffix(" GMT")
        .and_then(|time| {
            NaiveDateTime::parse_from_str(time, "%a, %d %b %Y %H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(time, "%A, %d-%b-%y %H:%M:%S"))
                .ok()
        })
        .or_else(|| NaiveDateTime::parse_from_str(value, "%a %b %e %H:%M:%S %Y").ok());
    fixdate.map(|time| time.and_utc()).ok_or_else(invalid)
}

/// The difference between the server's clock and the local clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClockOffset(TimeDelta);

impl ClockOffset {
    pub const ZERO: Self = Self(TimeDelta::zero());

    /// The offset implied by `server_time`, read from a response to a request
    /// sent at `request_sent` and completed at `response_received` (local
    /// clock). The server time is assumed to correspond to the midpoint of the
    /// exchange, which halves the error a one-way delay would cause.
    pub fn from_exchange(
        server_time: DateTime<Utc>,
        request_sent: DateTime<Utc>,
        response_received: DateTime<Utc>,
    ) -> Self {
        // A local clock that stepped backwards mid-request leaves no usable
        // midpoint; the receive time is the better estimate then.
        let local = if response_received >= request_sent {
            request_sent + (response_received - request_sent) / 2
        } else {
            response_received
        };
        Self(server_time - local)
    }

    /// The server's current time for a local time.
    ///
    /// Saturates to `local` only beyond chrono's representable range (about
    /// ±262 000 years), which no real offset reaches.
    pub fn apply(self, local: DateTime<Utc>) -> DateTime<Utc> {
        local.checked_add_signed(self.0).unwrap_or(local)
    }

    pub fn as_time_delta(self) -> TimeDelta {
        self.0
    }
}

/// A source of the current local time, injectable for tests.
pub trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when told to.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    pub fn set(&self, time: DateTime<Utc>) {
        *self.now.lock() = time;
    }

    /// Moves the clock forward; saturates at chrono's maximum time.
    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock();
        *now = TimeDelta::from_std(by)
            .ok()
            .and_then(|by| now.checked_add_signed(by))
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn document() -> Url {
        Url::parse("https://example.com/live/manifest.mpd").unwrap()
    }

    fn at(secs: i64, millis: u32) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, millis * 1_000_000).unwrap()
    }

    #[test]
    fn descriptors() {
        let timing = |scheme, value| UtcTiming::from_descriptor(scheme, value, &document());
        assert_eq!(
            timing(
                "urn:mpeg:dash:utc:http-iso:2014",
                Some("https://time.example.com/?iso /relative")
            ),
            Some(UtcTiming::HttpIso {
                urls: vec![
                    Url::parse("https://time.example.com/?iso").unwrap(),
                    Url::parse("https://example.com/relative").unwrap(),
                ]
            })
        );
        assert!(matches!(
            timing(
                "urn:mpeg:dash:utc:http-xsdate:2012",
                Some("https://t.example.com/")
            ),
            Some(UtcTiming::HttpXsDate { .. })
        ));
        assert!(matches!(
            timing(
                "urn:mpeg:dash:utc:http-head:2014",
                Some("https://t.example.com/")
            ),
            Some(UtcTiming::HttpHead { .. })
        ));
        assert_eq!(
            timing(
                "urn:mpeg:dash:utc:direct:2014",
                Some("2024-01-01T00:00:00Z")
            ),
            Some(UtcTiming::Direct {
                time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
            })
        );
        assert_eq!(
            timing("urn:mpeg:dash:utc:ntp:2014", Some("pool.ntp.org")),
            None
        );
        assert_eq!(
            timing("urn:mpeg:dash:utc:http-ntp:2014", Some("https://t/")),
            None
        );
        assert_eq!(timing("urn:example:clock", Some("https://t/")), None);
        assert_eq!(timing("urn:mpeg:dash:utc:http-iso:2014", None), None);
        assert_eq!(
            timing("urn:mpeg:dash:utc:http-iso:2014", Some("ftp://t/")),
            None
        );
        assert_eq!(timing("urn:mpeg:dash:utc:direct:2014", Some("now")), None);
    }

    #[test]
    fn response_bodies() {
        let expected = at(1_704_067_200, 123);
        for body in [
            &b"2024-01-01T00:00:00.123Z"[..],
            b"2024-01-01T00:00:00.123Z\r\n",
            b"2024-01-01T00:00:00.123",
        ] {
            assert_eq!(UtcTiming::parse_body(body).ok(), Some(expected));
        }
        assert!(UtcTiming::parse_body(b"1704067200").is_err());
        assert!(UtcTiming::parse_body(&[0xff]).is_err());
    }

    #[test]
    fn http_dates() {
        let expected = Utc.with_ymd_and_hms(1994, 11, 6, 8, 49, 37).unwrap();
        for value in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(parse_http_date(value).ok(), Some(expected), "{value}");
        }
        for value in [
            "",
            "06 Nov 1994",
            "Sun, 06 Nov 1994 08:49:37 PST",
            "1994-11-06T08:49:37Z",
        ] {
            assert!(parse_http_date(value).is_err(), "{value}");
        }
    }

    #[test]
    fn offset_uses_the_midpoint_of_the_exchange() {
        // Request at 100.000, response at 100.400, server says 105.200 → the
        // server time corresponds to local 100.200 → offset +5 s.
        let offset = ClockOffset::from_exchange(at(105, 200), at(100, 0), at(100, 400));
        assert_eq!(offset.as_time_delta(), TimeDelta::seconds(5));
        assert_eq!(offset.apply(at(200, 0)), at(205, 0));

        let behind = ClockOffset::from_exchange(at(90, 0), at(100, 0), at(100, 0));
        assert_eq!(behind.apply(at(200, 0)), at(190, 0));

        let stepped_back = ClockOffset::from_exchange(at(50, 0), at(100, 0), at(40, 0));
        assert_eq!(stepped_back.as_time_delta(), TimeDelta::seconds(10));
        assert_eq!(ClockOffset::ZERO.apply(at(1, 0)), at(1, 0));
    }
}
