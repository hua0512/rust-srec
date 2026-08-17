//! Utility modules for rust-srec.

use std::ffi::OsStr;

use tokio::process::Command;

pub mod filename;
pub mod fs;
pub mod http_client;
pub mod json;
pub(crate) mod task_supervisor;
pub mod text;
pub mod url;

pub(crate) fn configure_ffmpeg_locale(command: &mut Command) {
    let inherited_lc_all = std::env::var_os("LC_ALL");
    configure_ffmpeg_locale_with_lc_all(command, inherited_lc_all.as_deref());
}

fn configure_ffmpeg_locale_with_lc_all(command: &mut Command, inherited_lc_all: Option<&OsStr>) {
    // LC_ALL overrides category variables, so retain its Unicode-sensitive categories.
    if let Some(locale) = inherited_lc_all
        && !locale.is_empty()
    {
        command.env("LC_CTYPE", locale).env("LC_TIME", locale);
    }

    command
        .env_remove("LC_ALL")
        .env("LC_MESSAGES", "C")
        .env("LC_NUMERIC", "C");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_env<'a>(command: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
        command
            .as_std()
            .get_envs()
            .find(|(candidate, _)| *candidate == OsStr::new(key))
            .map(|(_, value)| value)
    }

    #[test]
    fn ffmpeg_locale_preserves_unicode_categories_from_lc_all() {
        let mut command = Command::new("ffmpeg");

        configure_ffmpeg_locale_with_lc_all(&mut command, Some(OsStr::new("C.UTF-8")));

        assert_eq!(configured_env(&command, "LC_ALL"), Some(None));
        assert_eq!(
            configured_env(&command, "LC_MESSAGES"),
            Some(Some(OsStr::new("C")))
        );
        assert_eq!(
            configured_env(&command, "LC_NUMERIC"),
            Some(Some(OsStr::new("C")))
        );
        assert_eq!(
            configured_env(&command, "LC_CTYPE"),
            Some(Some(OsStr::new("C.UTF-8")))
        );
        assert_eq!(
            configured_env(&command, "LC_TIME"),
            Some(Some(OsStr::new("C.UTF-8")))
        );
    }

    #[test]
    fn ffmpeg_locale_ignores_empty_lc_all() {
        let mut command = Command::new("ffmpeg");

        configure_ffmpeg_locale_with_lc_all(&mut command, Some(OsStr::new("")));

        assert_eq!(configured_env(&command, "LC_ALL"), Some(None));
        assert_eq!(configured_env(&command, "LC_CTYPE"), None);
        assert_eq!(configured_env(&command, "LC_TIME"), None);
    }
}
