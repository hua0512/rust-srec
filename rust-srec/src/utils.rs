//! Utility modules for rust-srec.

use tokio::process::Command;

pub mod filename;
pub mod fs;
pub mod http_client;
pub mod json;
pub(crate) mod task_supervisor;
pub mod text;
pub mod url;

pub(crate) fn configure_ffmpeg_locale(command: &mut Command) {
    // LC_ALL would override these categories and can break Unicode output paths.
    command
        .env_remove("LC_ALL")
        .env("LC_MESSAGES", "C")
        .env("LC_NUMERIC", "C");
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn ffmpeg_locale_keeps_unicode_categories_inherited() {
        let mut command = Command::new("ffmpeg");

        configure_ffmpeg_locale(&mut command);

        let envs: Vec<_> = command.as_std().get_envs().collect();
        assert!(
            envs.iter()
                .any(|(key, value)| *key == OsStr::new("LC_ALL") && value.is_none())
        );
        assert!(envs.iter().any(|(key, value)| {
            *key == OsStr::new("LC_MESSAGES") && *value == Some(OsStr::new("C"))
        }));
        assert!(envs.iter().any(|(key, value)| {
            *key == OsStr::new("LC_NUMERIC") && *value == Some(OsStr::new("C"))
        }));
        assert!(
            !envs.iter().any(|(key, _)| {
                *key == OsStr::new("LC_CTYPE") || *key == OsStr::new("LC_TIME")
            })
        );
    }
}
