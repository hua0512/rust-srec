//! Paths of the external tools the app runs.

/// The path to run an external tool from: `configured` when set, otherwise the
/// `env_var` environment variable, otherwise `default`.
pub(crate) fn resolve(configured: Option<&str>, env_var: &str, default: &str) -> String {
    configured
        .map(str::to_owned)
        .or_else(|| std::env::var(env_var).ok())
        .unwrap_or_else(|| default.to_owned())
}
