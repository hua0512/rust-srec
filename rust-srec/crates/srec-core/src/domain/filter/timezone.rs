//! One timezone policy for filter matching, wake boundaries and validation.
use jiff::tz::TimeZone;

#[derive(Debug, Clone)]
pub struct FilterTimezone(pub(super) TimeZone);

#[derive(Debug, thiserror::Error)]
#[error("'{0}' is not a valid IANA timezone or 'local'")]
pub struct InvalidFilterTimezone(String);

impl FilterTimezone {
    pub fn parse(value: Option<&str>) -> Result<Self, InvalidFilterTimezone> {
        match value {
            None => Ok(Self(TimeZone::UTC)),
            Some("local") => Ok(Self(TimeZone::system())),
            Some(value) => TimeZone::get(value)
                .map(Self)
                .map_err(|_| InvalidFilterTimezone(value.to_owned())),
        }
    }
}
