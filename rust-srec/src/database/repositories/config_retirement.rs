//! Configuration definitions retained by recordings being retired.

use sqlx::SqliteConnection;

#[derive(Clone, Copy)]
pub(crate) enum RetiredConfigKind {
    Template,
    JobPreset,
    PipelinePreset,
}

impl RetiredConfigKind {
    fn key(self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::JobPreset => "job_preset",
            Self::PipelinePreset => "pipeline_preset",
        }
    }

    fn table(self) -> &'static str {
        match self {
            Self::Template => "template_config",
            Self::JobPreset => "job_presets",
            Self::PipelinePreset => "pipeline_presets",
        }
    }
}

/// Delete an omitted definition or retain it until its recording owners settle.
/// The caller's import transaction owns both the streamer markers and this intent.
pub(crate) async fn delete_or_defer(
    conn: &mut SqliteConnection,
    kind: RetiredConfigKind,
    id: &str,
) -> Result<(), sqlx::Error> {
    let needed: bool = match kind {
        RetiredConfigKind::Template => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM streamers WHERE template_config_id = ?) OR EXISTS(SELECT 1 FROM live_sessions s JOIN credential_profiles p ON p.id = json_extract(s.credential_binding, '$.identity.profile_id') WHERE p.template_id = ? AND s.end_time IS NULL)",
            )
            .bind(id)
            .bind(id)
            .fetch_one(&mut *conn)
            .await?
        }
        // Presets can be referenced by name inside nested workflows. Retain the
        // omitted set until every retirement completes, including across restarts.
        _ => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM streamers WHERE deleted_at IS NOT NULL)",
            )
            .fetch_one(&mut *conn)
            .await?
        }
    };
    if needed {
        sqlx::query(
            "INSERT OR IGNORE INTO retirement_config_deletions(kind, config_id) VALUES (?, ?)",
        )
        .bind(kind.key())
        .bind(id)
        .execute(&mut *conn)
        .await?;
    } else {
        if matches!(kind, RetiredConfigKind::Template) {
            super::credential_profiles::delete_owner_profiles(
                conn,
                &crate::credentials::CredentialOwner::Template {
                    template_id: id.to_owned(),
                },
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        }
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {} WHERE id = ?",
            kind.table()
        )))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Runs in the transaction that reaps a streamer. Definition triggers remove
/// the corresponding intent, making physical deletion and intent removal atomic.
pub(crate) async fn reap(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    reap_retired_profiles(conn).await?;
    let template_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM template_config WHERE id IN (SELECT config_id FROM retirement_config_deletions WHERE kind = 'template') AND NOT EXISTS(SELECT 1 FROM streamers WHERE template_config_id = template_config.id) AND NOT EXISTS(SELECT 1 FROM live_sessions s JOIN credential_profiles p ON p.id = json_extract(s.credential_binding, '$.identity.profile_id') WHERE p.template_id = template_config.id AND s.end_time IS NULL)").fetch_all(&mut *conn).await?;
    for id in template_ids {
        super::credential_profiles::delete_owner_profiles(
            conn,
            &crate::credentials::CredentialOwner::Template {
                template_id: id.clone(),
            },
        )
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        sqlx::query("DELETE FROM template_config WHERE id = ?")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    for kind in [
        RetiredConfigKind::JobPreset,
        RetiredConfigKind::PipelinePreset,
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {} WHERE id IN (SELECT config_id FROM retirement_config_deletions WHERE kind = ?) AND NOT EXISTS(SELECT 1 FROM streamers WHERE deleted_at IS NOT NULL)", kind.table(),
        )))
        .bind(kind.key())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Remove only unreferenced retirement intents. Live sessions and policies keep
/// their profile rows until runtime finalization and owner reaping have settled.
pub(crate) async fn reap_retired_profiles(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT profile_id FROM retirement_credential_profiles")
            .fetch_all(&mut *conn)
            .await?;
    for id in ids {
        let references = super::credential_profiles::references(conn, &id, None)
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        if references.is_empty() {
            sqlx::query("DELETE FROM credential_profiles WHERE id = ?")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> sqlx::SqlitePool {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        sqlx::query("INSERT INTO template_config(id, name) VALUES ('retained', 'Retained')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state, deleted_at) VALUES ('retiring', 'Retiring', 'https://example.com/retiring', 'platform-huya', 'retained', 'NOT_LIVE', 1)")
            .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn editing_a_retained_definition_cancels_its_deletion() {
        let pool = fixture().await;
        let mut conn = pool.acquire().await.unwrap();
        delete_or_defer(&mut conn, RetiredConfigKind::Template, "retained")
            .await
            .unwrap();
        sqlx::query("UPDATE template_config SET name = 'Edited' WHERE id = 'retained'")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DELETE FROM streamers WHERE id = 'retiring'")
            .execute(&mut *conn)
            .await
            .unwrap();
        reap(&mut conn).await.unwrap();
        let name: String =
            sqlx::query_scalar("SELECT name FROM template_config WHERE id = 'retained'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(name, "Edited");
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retirement_config_deletions")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn reusing_a_retained_template_cancels_its_deletion() {
        let pool = fixture().await;
        let mut conn = pool.acquire().await.unwrap();
        delete_or_defer(&mut conn, RetiredConfigKind::Template, "retained")
            .await
            .unwrap();
        sqlx::query("INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state) VALUES ('active', 'Active', 'https://example.com/active', 'platform-huya', 'retained', 'NOT_LIVE')").execute(&mut *conn).await.unwrap();
        sqlx::query("DELETE FROM streamers WHERE id = 'retiring'")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE streamers SET template_config_id = NULL WHERE id = 'active'")
            .execute(&mut *conn)
            .await
            .unwrap();
        reap(&mut conn).await.unwrap();
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM template_config WHERE id = 'retained'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(remaining, 1);
    }
}
