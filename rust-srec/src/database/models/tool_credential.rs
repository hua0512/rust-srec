//! `tool_credentials` table model.
//!
//! Opt-in login material for external CLI tools, saved by a tool's login
//! endpoint (`remember` flag) and replayed for automatic re-login. The API
//! layer never serializes this model into a response; it only reports
//! `has_stored_credentials`.

use sqlx::FromRow;

/// One row from the `tool_credentials` table.
///
/// `(tool, account_key)` is unique. Both are free-form so a new tool reuses
/// the table without a schema change — the same convention as
/// `upload_records.uploader`.
#[derive(Debug, Clone, FromRow)]
pub struct ToolCredentialDbModel {
    /// Owning tool (`"baidupcs"` today).
    pub tool: String,
    /// Per-tool account discriminator; `""` is that tool's default account.
    /// For `baidupcs` this is the BaiduPCS-Go config directory (see
    /// `crate::baidupcs::config_dir_key`).
    pub account_key: String,
    /// Tool-shaped JSON payload (`crate::baidupcs::LoginMaterial` for
    /// `baidupcs`).
    pub payload: String,
    /// Milliseconds since Unix epoch (UTC).
    pub created_at: i64,
    pub updated_at: i64,
}
