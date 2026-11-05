//! Typed parsing of optional environment variables.

use std::env::VarError;
use std::fmt::Display;
use std::str::FromStr;

/// Read and parse `name`, returning `None` when it is unset or blank.
///
/// A value that is set but does not parse is a configuration error, so a typo fails startup
/// instead of silently running with the default.
pub(crate) fn parse<T>(name: &str) -> crate::Result<Option<T>>
where
    T: FromStr,
    T::Err: Display,
{
    match std::env::var(name) {
        Ok(value) => parse_value(name, &value),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(crate::Error::config(format!(
            "{name} must be valid Unicode"
        ))),
    }
}

fn parse_value<T>(name: &str, value: &str) -> crate::Result<Option<T>>
where
    T: FromStr,
    T::Err: Display,
{
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse()
        .map(Some)
        .map_err(|error| crate::Error::config(format!("{name}={value:?} is invalid: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_values_are_errors_and_blank_values_are_unset() {
        assert_eq!(
            parse_value::<u16>("API_PORT", " 8080 ").unwrap(),
            Some(8080)
        );
        assert_eq!(parse_value::<u16>("API_PORT", "  ").unwrap(), None);
        let error = parse_value::<u16>("API_PORT", "80800").unwrap_err();
        assert!(error.to_string().contains("API_PORT"), "{error}");
        assert!(parse_value::<u64>("MIN_PASSWORD_LENGTH", "eight").is_err());
    }
}
