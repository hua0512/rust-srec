//! One timezone policy for filter matching, wake boundaries and validation.
use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy)]
pub(crate) enum FilterTimezone {
    Named(Tz),
    Local,
}

#[derive(Debug, thiserror::Error)]
#[error("'{0}' is not a valid IANA timezone or 'local'")]
pub(crate) struct InvalidFilterTimezone(String);

impl FilterTimezone {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self, InvalidFilterTimezone> {
        match value {
            None => Ok(Self::Named(chrono_tz::UTC)),
            Some("local") => Ok(Self::Local),
            Some(value) => value
                .parse()
                .map(Self::Named)
                .map_err(|_| InvalidFilterTimezone(value.to_owned())),
        }
    }

    pub(crate) fn date(self, now: DateTime<Utc>) -> NaiveDate {
        match self {
            Self::Named(timezone) => now.with_timezone(&timezone).date_naive(),
            Self::Local => now.with_timezone(&Local).date_naive(),
        }
    }

    pub(crate) fn resolve(self, wall: NaiveDateTime) -> LocalResult<DateTime<Utc>> {
        match self {
            Self::Named(timezone) => timezone
                .from_local_datetime(&wall)
                .map(|time| time.with_timezone(&Utc)),
            Self::Local => Local
                .from_local_datetime(&wall)
                .map(|time| time.with_timezone(&Utc)),
        }
    }
}
