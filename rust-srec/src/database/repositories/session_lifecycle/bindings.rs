//! Active-session lookup helpers on `SessionLifecycleRepository`.

use super::SessionLifecycleRepository;
use crate::Result;

impl SessionLifecycleRepository {
    #[cfg(test)]
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

    /// Check whether `session_id` still has an open (`end_time IS NULL`) row.
    pub async fn is_session_active(&self, session_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM live_sessions WHERE id = ? AND end_time IS NULL)",
        )
        .bind(session_id)
        .fetch_one(&self.write_pool)
        .await?)
    }
}
