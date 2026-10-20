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
//! At startup, [`crate::backend::ServiceContainer`] reads the `RUST_SREC_LOCALE`
//! environment variable (default `"en"`) and calls [`set_locale`]. Supported
//! locales are determined by the YAML files under `rust-srec/locales/`.
//!
//! ## Why a wrapper module
//!
//! `rust_i18n::i18n!` reads the YAML files at compile time and needs to be
//! invoked once at the crate root. This module re-exports `t!` so downstream
//! modules can `use crate::i18n::t;`.

// `rust_i18n::i18n!("locales", ...)` is invoked at the crate root in `lib.rs`,
// not here, because the `t!` macro generates code that resolves `_rust_i18n_t`
// at `crate::_rust_i18n_t`. Re-exporting the macro from this module so call
// sites can stay focused: `use crate::i18n::t`.

pub use rust_i18n::t;

/// Localize a message and return it as an owned `String`.
///
/// Wraps [`rust_i18n::t!`] and unwraps the returned `Cow<'static, str>` via
/// [`Cow::into_owned`](std::borrow::Cow::into_owned), so interpolated results
/// (the common case for our
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

/// Localize a message in an explicit locale, ignoring the process-wide one.
///
/// Used where the reader's language is not a property of the process: a
/// notification channel carries its own locale, and two channels can be
/// delivering the same event in different languages at the same time, so
/// [`set_locale`] cannot express it.
///
/// ```ignore
/// let title: String = crate::t_str_in!("zh-CN", "notification.stream_online.title",
///     streamer_name = name.as_str());
/// ```
///
/// `locale` is a fixed leading argument rather than a `locale = …` pair so the
/// expansion writes that pair itself; forwarding it as a captured fragment
/// relies on how `rust_i18n::t!` parses its arguments.
#[macro_export]
macro_rules! t_str_in {
    ($locale:expr, $key:expr $(,)?) => {
        ::rust_i18n::t!($key, locale = $locale).into_owned()
    };
    ($locale:expr, $key:expr, $($k:ident = $v:expr),+ $(,)?) => {
        ::rust_i18n::t!($key, locale = $locale, $($k = $v),+).into_owned()
    };
}

// `#[macro_export]` provides the crate-root paths; these re-exports also
// support module-qualified paths under `crate::i18n`.
pub use crate::{t_str, t_str_in};

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
