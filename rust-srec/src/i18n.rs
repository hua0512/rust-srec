//! Backend internationalization.
//!
//! This module wires up [`rust_i18n`] for backend-emitted user-facing strings.
//! All [`crate::notification::events::NotificationEvent`] variants — including
//! the nested credential events — resolve their titles and descriptions
//! through [`t!`] and [`crate::t_str!`] against the per-locale YAML files
//! under `rust-srec/locales/`. API error payloads and health-component
//! messages remain English: the frontend has its own Lingui catalog and
//! API consumers are better served by stable English codes.
//!
//! ## Locale selection
//!
//! At startup, [`crate::services::container`] reads the `RUST_SREC_LOCALE`
//! environment variable (default `"en"`) and calls [`set_locale`]. Supported
//! locales are determined by the YAML files under `rust-srec/locales/`.
//!
//! ## Why a wrapper module
//!
//! `rust_i18n::i18n!` reads the YAML files at compile time and needs to be
//! invoked exactly once per crate, in scope where the `t!` macro will be used.
//! Centralizing it here means downstream modules just `use crate::i18n::t;`
//! instead of duplicating the macro invocation or importing `rust_i18n` directly.

// `rust_i18n::i18n!("locales", ...)` is invoked at the crate root in `lib.rs`,
// not here, because the `t!` macro generates code that resolves `_rust_i18n_t`
// at `crate::_rust_i18n_t`. Re-exporting the macro from this module so call
// sites can stay focused: `use crate::i18n::t`.

pub use rust_i18n::t;

/// Localize a message and return it as an owned `String`.
///
/// Wraps [`rust_i18n::t!`] and unwraps the returned `Cow<'static, str>` via
/// [`Cow::into_owned`], so interpolated results (the common case for our
/// keys, which all carry `%{...}` placeholders) are moved out directly and
/// only the rare static-fallback path clones.
///
/// ```ignore
/// let title: String = crate::t_str!("notification.stream_online.title");
/// let desc: String = crate::t_str!(
///     "notification.download_error.description.recoverable",
///     error_message = err.as_str(),
/// );
/// ```
///
/// Accepts the same placeholder syntax as `rust_i18n::t!`.
#[macro_export]
macro_rules! t_str {
    ($key:expr $(,)?) => {
        ::rust_i18n::t!($key).into_owned()
    };
    ($key:expr, $($k:ident = $v:expr),+ $(,)?) => {
        ::rust_i18n::t!($key, $($k = $v),+).into_owned()
    };
}

// Re-export so the `crate::t_str!(...)` form works from any module without
// an explicit `use`. `#[macro_export]` alone places the macro at the crate
// root; this re-export keeps the `crate::i18n::t_str` path working too for
// callers that prefer module-qualified paths.
pub use crate::t_str;

/// Set the active locale for backend-emitted notification strings.
///
/// Falls back to `"en"` if the requested locale is not present in the embedded
/// YAML files. Safe to call multiple times.
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

/// Read `RUST_SREC_LOCALE` from the environment and apply it.
///
/// Called once at container startup. If the variable is unset or empty, the
/// default locale (`"en"`) remains active.
pub fn init_from_env() {
    if let Ok(locale) = std::env::var("RUST_SREC_LOCALE")
        && !locale.trim().is_empty()
    {
        set_locale(locale.trim());
        tracing::info!("Backend locale set to {}", locale.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `rust_i18n::set_locale` mutates a process-global, not a thread-local.
    /// Cargo runs unit tests in parallel by default, so any test that switches
    /// locale must hold this mutex to avoid racing other locale-switching tests.
    static LOCALE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn english_title_resolves() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("en");
        let title = t!("notification.output_path_inaccessible.title", path = "/rec");
        assert!(title.contains("/rec"), "got: {}", title);
        assert!(title.contains("Output path inaccessible"), "got: {}", title);
    }

    #[test]
    fn chinese_title_resolves() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("zh-CN");
        let title = t!("notification.output_path_inaccessible.title", path = "/rec");
        assert!(title.contains("/rec"), "got: {}", title);
        // "输出路径无法写入" — verify Chinese localization is wired up
        assert!(title.contains("输出路径"), "got: {}", title);
        set_locale("en");
    }

    #[test]
    fn unknown_locale_falls_back_to_english() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("xx-YY");
        let title = t!("notification.output_path_inaccessible.title", path = "/rec");
        assert!(title.contains("Output path inaccessible"), "got: {}", title);
        set_locale("en");
    }

    #[test]
    fn description_branches_resolve_in_both_locales() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        for locale in ["en", "zh-CN"] {
            set_locale(locale);
            for kind in [
                "not_found",
                "storage_full",
                "permission_denied",
                "read_only",
                "timed_out",
                "other",
            ] {
                let key = format!("notification.output_path_inaccessible.description.{}", kind);
                let body = t!(&key, path = "/rec", kind = "ENOENT");
                assert!(
                    !body.is_empty() && !body.starts_with("notification."),
                    "missing translation for {} in {}: {:?}",
                    key,
                    locale,
                    body
                );
            }
        }
        set_locale("en");
    }

    #[test]
    fn available_locales_includes_en_and_zh_cn() {
        let locales = rust_i18n::available_locales!();
        assert!(locales.contains(&"en"), "missing en in {:?}", locales);
        assert!(locales.contains(&"zh-CN"), "missing zh-CN in {:?}", locales);
    }

    // ---------- t_str! macro ----------

    /// `t_str!` must produce an owned `String` — the type is load-bearing:
    /// the notification accessors return `String`, not `Cow<_>`.
    #[test]
    fn t_str_returns_string() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("en");
        let out: String =
            crate::t_str!("notification.output_path_inaccessible.title", path = "/rec");
        assert!(out.contains("/rec"));
    }

    /// Key-only form (no placeholders) compiles and returns the localized
    /// value. Not all of our keys need placeholders.
    #[test]
    fn t_str_key_only_form() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("en");
        // A key we know doesn't exist — should fall back to the key literal.
        let out: String = crate::t_str!("nonexistent.key.for.test");
        assert_eq!(out, "nonexistent.key.for.test");
    }

    /// Multiple placeholders + trailing comma — both must parse through the
    /// macro arm correctly. Trailing-comma ergonomics matter for rustfmt.
    #[test]
    fn t_str_multi_placeholder_with_trailing_comma() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("en");
        let out: String = crate::t_str!(
            "notification.output_path_inaccessible.description.other",
            path = "/rec",
            kind = "enoent",
        );
        assert!(out.contains("/rec"));
        assert!(out.contains("enoent"));
    }

    /// `t_str!` respects the current locale just like `t!` does — no
    /// accidental hardcoding to the source locale.
    #[test]
    fn t_str_follows_active_locale() {
        let _g = LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_locale("zh-CN");
        let out: String =
            crate::t_str!("notification.output_path_inaccessible.title", path = "/rec");
        assert!(out.contains("输出路径"), "got: {}", out);
        set_locale("en");
    }
}
