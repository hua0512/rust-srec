//! Credential store repository (SQLx).
//!
//! This is the database-backed persistence implementation for the credentials subsystem.

use async_trait::async_trait;
use sqlx::SqlitePool;
use tracing::{debug, instrument};

use crate::credentials::{
    CredentialError, CredentialScope, CredentialSource, CredentialStore, RefreshedCredentials,
};

/// SQLx-backed credential store.
pub struct SqlxCredentialStore {
    pool: SqlitePool,
}

impl SqlxCredentialStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn update_platform_credentials(
        &self,
        platform_id: &str,
        credentials: &RefreshedCredentials,
    ) -> Result<(), CredentialError> {
        debug!(platform_id = %platform_id, "Updating platform credentials");

        sqlx::query(
            r#"
            UPDATE platform_config
            SET cookies = ?
            WHERE id = ?
            "#,
        )
        .bind(&credentials.cookies)
        .bind(platform_id)
        .execute(&self.pool)
        .await?;

        // Update refresh_token, access_token, and last_cookie_check_* in platform_specific_config JSON.
        if credentials.refresh_token.is_some() || credentials.access_token.is_some() {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

            // Build the JSON update incrementally
            let mut json_expr = "COALESCE(platform_specific_config, '{}')".to_string();
            let mut binds: Vec<String> = Vec::new();

            if let Some(ref token) = credentials.refresh_token {
                json_expr = format!("json_set({}, '$.refresh_token', ?)", json_expr);
                binds.push(token.clone());
            }
            if let Some(ref token) = credentials.access_token {
                json_expr = format!("json_set({}, '$.access_token', ?)", json_expr);
                binds.push(token.clone());
            }

            json_expr = format!("json_set({}, '$.last_cookie_check_date', ?)", json_expr);
            binds.push(today);
            json_expr = format!("json_set({}, '$.last_cookie_check_result', 'valid')", json_expr);

            let sql = format!(
                "UPDATE platform_config SET platform_specific_config = {} WHERE id = ?",
                json_expr
            );

            let mut query = sqlx::query(&sql);
            for bind in &binds {
                query = query.bind(bind);
            }
            query = query.bind(platform_id);
            query.execute(&self.pool).await?;
        }

        debug!("Platform credentials updated successfully");
        Ok(())
    }

    async fn update_template_credentials(
        &self,
        template_id: &str,
        platform_name: &str,
        credentials: &RefreshedCredentials,
    ) -> Result<(), CredentialError> {
        debug!(template_id = %template_id, "Updating template credentials");

        let now = crate::database::time::now_ms();

        let overrides_to_store = if credentials.refresh_token.is_some()
            || credentials.access_token.is_some()
        {
            let existing_overrides: Option<String> = sqlx::query_scalar(
                r#"
                SELECT platform_overrides
                FROM template_config
                WHERE id = ?
                "#,
            )
            .bind(template_id)
            .fetch_one(&self.pool)
            .await?;

            let mut overrides: serde_json::Value = match existing_overrides.as_deref() {
                Some(s) if !s.trim().is_empty() => serde_json::from_str(s)?,
                _ => serde_json::Value::Object(serde_json::Map::new()),
            };

            let root = overrides.as_object_mut().ok_or_else(|| {
                CredentialError::Internal(
                    "template_config.platform_overrides must be a JSON object".to_string(),
                )
            })?;

            let entry = root
                .entry(platform_name.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            let platform_obj = entry.as_object_mut().ok_or_else(|| {
                CredentialError::Internal(format!(
                    "template_config.platform_overrides['{platform_name}'] must be a JSON object"
                ))
            })?;

            if let Some(ref token) = credentials.refresh_token {
                platform_obj.insert(
                    "refresh_token".to_string(),
                    serde_json::Value::String(token.clone()),
                );
            }
            if let Some(ref token) = credentials.access_token {
                platform_obj.insert(
                    "access_token".to_string(),
                    serde_json::Value::String(token.clone()),
                );
            }

            Some(serde_json::to_string(&overrides)?)
        } else {
            None
        };

        // Update cookies + (optional) refresh_token atomically.
        match overrides_to_store {
            Some(overrides_json) => {
                sqlx::query(
                    r#"
                    UPDATE template_config
                    SET cookies = ?,
                        platform_overrides = ?,
                        updated_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&credentials.cookies)
                .bind(overrides_json)
                .bind(now)
                .bind(template_id)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query(
                    r#"
                    UPDATE template_config
                    SET cookies = ?,
                        updated_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&credentials.cookies)
                .bind(now)
                .bind(template_id)
                .execute(&self.pool)
                .await?;
            }
        }

        debug!("Template credentials updated successfully");
        Ok(())
    }

    async fn update_streamer_credentials(
        &self,
        streamer_id: &str,
        credentials: &RefreshedCredentials,
    ) -> Result<(), CredentialError> {
        debug!(streamer_id = %streamer_id, "Updating streamer credentials");

        let now = crate::database::time::now_ms();

        sqlx::query(
            r#"
            UPDATE streamers
            SET streamer_specific_config = json_set(
                COALESCE(streamer_specific_config, '{}'),
                '$.cookies',
                ?
            ),
            updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&credentials.cookies)
        .bind(now)
        .bind(streamer_id)
        .execute(&self.pool)
        .await?;

        if let Some(ref token) = credentials.refresh_token {
            sqlx::query(
                r#"
                UPDATE streamers
                SET streamer_specific_config = json_set(
                    streamer_specific_config,
                    '$.refresh_token',
                    ?
                )
                WHERE id = ?
                "#,
            )
            .bind(token)
            .bind(streamer_id)
            .execute(&self.pool)
            .await?;
        }

        if let Some(ref token) = credentials.access_token {
            sqlx::query(
                r#"
                UPDATE streamers
                SET streamer_specific_config = json_set(
                    streamer_specific_config,
                    '$.access_token',
                    ?
                )
                WHERE id = ?
                "#,
            )
            .bind(token)
            .bind(streamer_id)
            .execute(&self.pool)
            .await?;
        }

        debug!("Streamer credentials updated successfully");
        Ok(())
    }
}

#[async_trait]
impl CredentialStore for SqlxCredentialStore {
    #[instrument(
        skip(self, credentials),
        fields(scope = %source.scope.describe(), platform = %source.platform_name)
    )]
    async fn update_credentials(
        &self,
        source: &CredentialSource,
        credentials: &RefreshedCredentials,
    ) -> Result<(), CredentialError> {
        match &source.scope {
            CredentialScope::Platform { platform_id, .. } => {
                self.update_platform_credentials(platform_id, credentials)
                    .await
            }
            CredentialScope::Template { template_id, .. } => {
                self.update_template_credentials(template_id, &source.platform_name, credentials)
                    .await
            }
            CredentialScope::Streamer { streamer_id, .. } => {
                self.update_streamer_credentials(streamer_id, credentials)
                    .await
            }
        }
    }

    #[instrument(skip(self), fields(scope = %scope.describe()))]
    async fn update_check_result(
        &self,
        scope: &CredentialScope,
        result: &str,
    ) -> Result<(), CredentialError> {
        // For now, only persist check results at platform level.
        if let CredentialScope::Platform { platform_id, .. } = scope {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            sqlx::query(
                r#"
                UPDATE platform_config
                SET platform_specific_config = json_set(
                    json_set(
                        COALESCE(platform_specific_config, '{}'),
                        '$.last_cookie_check_date',
                        ?
                    ),
                    '$.last_cookie_check_result',
                    ?
                )
                WHERE id = ?
                "#,
            )
            .bind(&today)
            .bind(result)
            .bind(platform_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}
