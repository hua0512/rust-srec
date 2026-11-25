//! Backend internationalization.
//!
//! This module wires up [`rust_i18n`] for backend-emitted notification strings.
//! All [`crate::notification::events::NotificationEvent`] variants — including
//! nested credential events — resolve their titles and descriptions through
//! `t_str_in!` against the per-locale YAML files under `rust-srec/locales/`.
//! API error payloads and health-component messages remain English: the
//! frontend has its own Lingui catalog and API consumers are better served by
//! stable English codes.
//!
//! ## Locale selection
//!
//! At startup, [`crate::backend::ServiceContainer`] reads the `RUST_SREC_LOCALE`
//! environment variable (default `"en"`) and calls [`set_locale`]. Supported
//! locales are determined by the YAML files under `rust-srec/locales/`.

// `rust_i18n::i18n!("locales", ...)` is invoked at the crate root in `lib.rs`,
// not here, because the `rust_i18n::t!` macro resolves `_rust_i18n_t` at
// `crate::_rust_i18n_t`.
//
// `rust_i18n::i18n!` reads `locales/` at compile time without telling rustc,
// so a locale edit would otherwise leave stale translations in the build.
// Including each file registers it as a compile input. Every file in
// `locales/` must be listed here.
const _: &str = include_str!("../locales/en.yml");
const _: &str = include_str!("../locales/zh-CN.yml");

/// Localize a notification message in an explicit locale, ignoring the
/// process-wide one.
///
/// Used where the reader's language is not a property of the process: a
/// notification channel carries its own locale, and two channels can be
/// delivering the same event in different languages at the same time, so
/// [`set_locale`] cannot express it.
///
/// `locale` is a fixed leading argument rather than a `locale = …` pair so the
/// expansion writes that pair itself; forwarding it as a captured fragment
/// relies on how `rust_i18n::t!` parses its arguments.
macro_rules! t_str_in {
    ($locale:expr, $key:expr $(,)?) => {
        ::rust_i18n::t!($key, locale = $locale).into_owned()
    };
    ($locale:expr, $key:expr, $($k:ident = $v:expr),+ $(,)?) => {
        ::rust_i18n::t!($key, locale = $locale, $($k = $v),+).into_owned()
    };
}

pub(crate) use t_str_in;

/// The locale currently applied by [`set_locale`].
///
/// The fallback for anything that has not been given a locale of its own.
pub fn current_locale() -> String {
    rust_i18n::locale().to_string()
}

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
