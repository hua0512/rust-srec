//! Credential-binding and active-session timestamp lookups on `SessionLifecycleRepository`.

use jiff::Timestamp;

use super::SessionLifecycleRepository;
use crate::Result;

impl SessionLifecycleRepository {
    pub(crate) async fn active_session_started_at(
        &self,
        session_id: &str,
    ) -> Result<Option<Timestamp>> {
        let started: Option<i64> = sqlx::query_scalar(
            "SELECT start_time FROM live_sessions WHERE id = ? AND end_time IS NULL",
        )
        .bind(session_id)
        .fetch_optional(&self.write_pool)
        .await?;
        Ok(started.and_then(|ms| Timestamp::from_millisecond(ms).ok()))
    }

    pub(crate) async fn session_started_at(&self, session_id: &str) -> Result<Option<Timestamp>> {
        let started: Option<i64> =
            sqlx::query_scalar("SELECT start_time FROM live_sessions WHERE id = ?")
                .bind(session_id)
                .fetch_optional(&self.write_pool)
                .await?;
        Ok(started.and_then(|ms| Timestamp::from_millisecond(ms).ok()))
    }

    pub(crate) async fn credential_binding_is_retiring(
        &self,
        session_id: &str,
        binding: &crate::credentials::CredentialBinding,
    ) -> Result<bool> {
        let crate::credentials::CredentialIdentity::Profile { profile_id } = &binding.identity
        else {
            return Ok(false);
        };
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM live_sessions s JOIN retirement_credential_profiles r ON r.profile_id = json_extract(s.credential_binding, '$.identity.profile_id') WHERE s.id = ? AND s.end_time IS NULL AND r.profile_id = ? AND json_extract(s.credential_binding, '$.epoch') = ?)").bind(session_id).bind(profile_id).bind(binding.epoch as i64).fetch_one(&self.write_pool).await?)
    }

    /// The credential binding of `session_id` while that session is open.
    pub(crate) async fn active_session_credential_binding(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::credentials::CredentialBinding>> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT credential_binding FROM live_sessions WHERE id = ? AND end_time IS NULL",
        )
        .bind(session_id)
        .fetch_optional(&self.write_pool)
        .await?
        .flatten();
        Ok(raw.as_deref().map(serde_json::from_str).transpose()?)
    }

    /// The streamer's open session whose bound profile is being retired.
    pub(crate) async fn retiring_credential_session(
        &self,
        streamer_id: &str,
    ) -> Result<Option<(String, crate::credentials::CredentialBinding)>> {
        let row: Option<(String, String)> = sqlx::query_as("SELECT s.id, s.credential_binding FROM live_sessions s JOIN retirement_credential_profiles r ON r.profile_id = json_extract(s.credential_binding, '$.identity.profile_id') WHERE s.streamer_id = ? AND s.end_time IS NULL").bind(streamer_id).fetch_optional(&self.write_pool).await?;
        row.map(|(id, raw)| serde_json::from_str(&raw).map(|binding| (id, binding)))
            .transpose()
            .map_err(Into::into)
    }

    /// The id and credential binding of the streamer's open session.
    pub(crate) async fn active_streamer_session_binding(
        &self,
        streamer_id: &str,
    ) -> Result<Option<(String, Option<crate::credentials::CredentialBinding>)>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as("SELECT id, credential_binding FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL").bind(streamer_id).fetch_optional(&self.write_pool).await?;
        row.map(|(id, raw)| {
            raw.as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map(|binding| (id, binding))
        })
        .transpose()
        .map_err(Into::into)
    }

    pub(crate) async fn commit_credential_binding(
        &self,
        session_id: String,
        streamer_id: String,
        binding: crate::credentials::CredentialBinding,
    ) -> Result<crate::credentials::CredentialBinding> {
        self.write_transaction("commit recording credential binding", move |tx| {
            Box::pin(async move {
                super::super::credential_profiles::commit_session_binding(
                    tx,
                    &session_id,
                    &streamer_id,
                    &binding,
                    true,
                )
                .await
            })
        })
        .await
    }

    /// Resolve terminal events after the in-memory retention window expires.
    pub async fn is_session_active(&self, session_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM live_sessions WHERE id = ? AND end_time IS NULL)",
        )
        .bind(session_id)
        .fetch_one(&self.write_pool)
        .await?)
    }
}
