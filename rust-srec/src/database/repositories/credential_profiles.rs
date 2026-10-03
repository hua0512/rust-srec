//! Transactional account storage and the single policy-reference graph.

use sqlx::{SqliteConnection, SqlitePool};
use std::sync::{Arc, OnceLock};

use crate::credentials::{
    CredentialMaterial, CredentialOwner, CredentialProfile, CredentialProfileHealth,
    CredentialSelection, ProfileError,
};
use crate::database::begin_immediate;
use crate::{Error, Result};

type CredentialPublication = Arc<dyn Fn(CredentialOwner) + Send + Sync>;

#[derive(Clone)]
pub struct CredentialProfileRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
    supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
    publication: Arc<OnceLock<CredentialPublication>>,
    material_publication: Arc<OnceLock<CredentialPublication>>,
    admission: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    commit_gate:
        Arc<parking_lot::RwLock<Option<Arc<crate::database::committed_writer::CommitTestGate>>>>,
}

impl CredentialProfileRepository {
    pub(crate) async fn mark_invalid(&self, source: &CredentialProfile) -> Result<bool> {
        let mut transaction = begin_immediate(&self.write_pool).await?;
        let current = load(&mut transaction, &source.id).await?;
        require_current(&mut transaction, source, &current).await?;
        let changed = sqlx::query("INSERT INTO credential_profile_health(profile_id, revision, validity, reason_code) VALUES (?, ?, 'invalid', 'login_required') ON CONFLICT(profile_id) DO UPDATE SET revision = excluded.revision, validity = 'invalid', reason_code = excluded.reason_code WHERE credential_profile_health.revision != excluded.revision OR credential_profile_health.validity != 'invalid'").bind(&source.id).bind(source.revision).execute(&mut *transaction).await?.rows_affected() > 0;
        transaction.commit().await?;
        Ok(changed)
    }
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self {
            pool,
            write_pool,
            supervisor: Arc::new(
                crate::utils::task_supervisor::TaskSupervisor::for_committed_work(),
            ),
            publication: Arc::new(OnceLock::new()),
            material_publication: Arc::new(OnceLock::new()),
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            #[cfg(test)]
            commit_gate: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    async fn after_commit(&self) {
        #[cfg(test)]
        {
            let gate = self.commit_gate.read().clone();
            if let Some(gate) = gate {
                gate.started.notify_one();
                gate.release.notified().await;
            }
        }
    }

    pub(crate) fn with_supervisor(
        mut self,
        supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
    ) -> Self {
        self.supervisor = supervisor;
        self
    }

    pub(crate) fn bind_publication(&self, publication: CredentialPublication) {
        self.publication.get_or_init(|| publication);
    }

    pub(crate) fn publish_owner(&self, owner: CredentialOwner) {
        if let Some(publish) = self.publication.get() {
            publish(owner);
        }
    }

    pub(crate) fn bind_material_publication(&self, publication: CredentialPublication) {
        self.material_publication.get_or_init(|| publication);
    }

    pub(crate) fn publish_material(&self, owner: CredentialOwner) {
        if let Some(publish) = self.material_publication.get() {
            publish(owner);
        } else {
            self.publish_owner(owner);
        }
    }

    pub async fn platform_name(&self, id: &str) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT platform_name FROM platform_config WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Display names for notifications. A concurrently removed owner keeps its ID.
    pub async fn scope(
        &self,
        owner: &CredentialOwner,
    ) -> Result<crate::credentials::CredentialScope> {
        use crate::credentials::CredentialScope;
        let query = match owner {
            CredentialOwner::Platform { .. } => {
                "SELECT platform_name FROM platform_config WHERE id = ?"
            }
            CredentialOwner::Template { .. } => "SELECT name FROM template_config WHERE id = ?",
            CredentialOwner::Streamer { .. } => "SELECT name FROM streamers WHERE id = ?",
        };
        let name: Option<String> = sqlx::query_scalar(query)
            .bind(owner.id())
            .fetch_optional(&self.pool)
            .await?;
        let name = name.unwrap_or_else(|| owner.id().to_owned());
        Ok(match owner {
            CredentialOwner::Platform { platform_id } => CredentialScope::Platform {
                platform_id: platform_id.clone(),
                platform_name: name,
            },
            CredentialOwner::Template { template_id } => CredentialScope::Template {
                template_id: template_id.clone(),
                template_name: name,
            },
            CredentialOwner::Streamer { streamer_id } => CredentialScope::Streamer {
                streamer_id: streamer_id.clone(),
                streamer_name: name,
            },
        })
    }

    pub async fn active_binding(
        &self,
        streamer_id: &str,
    ) -> Result<Option<crate::credentials::CredentialBinding>> {
        let raw: Option<Option<String>> = sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL")
            .bind(streamer_id).fetch_optional(&self.pool).await?;
        raw.flatten()
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(Into::into)
    }

    pub async fn validate_current(&self, source: &CredentialProfile) -> Result<()> {
        let mut connection = self.pool.acquire().await?;
        let current = load(&mut connection, &source.id).await?;
        require_current(&mut connection, source, &current).await
    }

    pub async fn validate_selection(
        &self,
        policy: &crate::credentials::ResolvedCredentialPolicy,
    ) -> Result<()> {
        validate_policy(
            &mut *self.pool.acquire().await?,
            &policy.owner,
            &policy.platform_id,
            &policy.selection,
        )
        .await
    }

    pub async fn record_refresh_failure(&self, source: &CredentialProfile) -> Result<(i64, bool)> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, &source.id).await?;
        require_current(&mut tx, source, &current).await?;
        let now = crate::database::time::now_ms();
        let count: i64 = sqlx::query_scalar("INSERT INTO credential_profile_health(profile_id, revision, validity, refresh_failure_count, last_failure_at) VALUES (?, ?, 'needs_refresh', 1, ?) ON CONFLICT(profile_id) DO UPDATE SET refresh_failure_count = CASE WHEN credential_profile_health.last_failure_at < ? OR credential_profile_health.revision != excluded.revision THEN 1 ELSE credential_profile_health.refresh_failure_count + 1 END, last_failure_at = excluded.last_failure_at, revision = excluded.revision RETURNING refresh_failure_count")
            .bind(&source.id).bind(source.revision).bind(now).bind(now - 6 * 60 * 60 * 1000).fetch_one(&mut *tx).await?;
        let notify = count == 1 || count % 3 == 0;
        if notify {
            sqlx::query("UPDATE credential_profile_health SET last_notified_failure_count = ? WHERE profile_id = ?").bind(count).bind(&source.id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok((count, notify))
    }

    pub async fn get(&self, id: &str) -> Result<CredentialProfile> {
        load(&mut *self.pool.acquire().await?, id).await
    }

    pub async fn accessible(
        &self,
        owner: &CredentialOwner,
        platform_id: &str,
    ) -> Result<Vec<CredentialProfile>> {
        let mut connection = self.pool.acquire().await?;
        require_owner(&mut connection, owner, platform_id).await?;
        let profiles = sqlx::query_as::<_, CredentialProfile>("SELECT * FROM credential_profiles WHERE platform_config_id = ? ORDER BY created_at, id")
            .bind(platform_id).fetch_all(&mut *connection).await?;
        let mut accessible = Vec::new();
        for profile in profiles {
            if can_access(&mut connection, owner, &profile).await? {
                accessible.push(profile);
            }
        }
        Ok(accessible)
    }

    pub async fn create(
        &self,
        owner: &CredentialOwner,
        platform_id: &str,
        label: &str,
        enabled: bool,
        material: &CredentialMaterial,
    ) -> Result<CredentialProfile> {
        let repository = self.clone();
        let owner = owner.clone();
        let platform_id = platform_id.to_owned();
        let label = label.to_owned();
        let material = material.clone();
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Other("Credential writer is shutting down".into()))?;
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let _permit = permit;
            let profile = repository
                .create_inner(&owner, &platform_id, &label, enabled, &material)
                .await?;
            repository.publish_material(owner);
            Ok(profile)
        })
        .await
    }

    async fn create_inner(
        &self,
        owner: &CredentialOwner,
        platform_id: &str,
        label: &str,
        enabled: bool,
        material: &CredentialMaterial,
    ) -> Result<CredentialProfile> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let profile = create_in(&mut tx, owner, platform_id, label, enabled, material).await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.after_commit().await;
        Ok(profile)
    }

    pub async fn update(
        &self,
        id: &str,
        expected_version: i64,
        label: Option<&str>,
        enabled: Option<bool>,
        replacement: Option<&CredentialMaterial>,
    ) -> Result<CredentialProfile> {
        let repository = self.clone();
        let id = id.to_owned();
        let label = label.map(str::to_owned);
        let replacement = replacement.cloned();
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Other("Credential writer is shutting down".into()))?;
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let _permit = permit;
            let profile = repository
                .update_inner(
                    &id,
                    expected_version,
                    label.as_deref(),
                    enabled,
                    replacement.as_ref(),
                )
                .await?;
            repository.publish_material(profile.owner()?);
            Ok(profile)
        })
        .await
    }

    async fn update_inner(
        &self,
        id: &str,
        expected_version: i64,
        label: Option<&str>,
        enabled: Option<bool>,
        replacement: Option<&CredentialMaterial>,
    ) -> Result<CredentialProfile> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, id).await?;
        if current.version != expected_version {
            return Err(ProfileError::StaleVersion.into());
        }
        require_not_retiring(&mut tx, id).await?;
        let owner = current.owner()?;
        let platform = require_owner(&mut tx, &owner, &current.platform_config_id).await?;
        let label = validate_label(label.unwrap_or(&current.label))?;
        if let Some(material) = replacement {
            material.validate(&platform)?;
        }
        let material = replacement.cloned().unwrap_or(current.material()?);
        let enabled = enabled.unwrap_or(current.enabled);
        let changed = replacement.is_some() || enabled != current.enabled;
        sqlx::query("UPDATE credential_profiles SET label = ?, enabled = ?, cookies = ?, refresh_token = ?, access_token = ?, reauth_config = ?, revision = revision + ?, version = version + 1, updated_at = ? WHERE id = ? AND version = ?")
            .bind(label).bind(enabled).bind(&material.cookies).bind(&material.refresh_token).bind(&material.access_token)
            .bind(material.reauth_config.as_ref().map(serde_json::to_string).transpose()?).bind(i64::from(changed))
            .bind(crate::database::time::now_ms()).bind(id).bind(expected_version).execute(&mut *tx).await?;
        if changed {
            sqlx::query("DELETE FROM credential_profile_health WHERE profile_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let updated = load(&mut tx, id).await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.after_commit().await;
        Ok(updated)
    }

    /// Provider replacement preserves absent tokens; manual replacement clears them.
    pub async fn refreshed(
        &self,
        source: &CredentialProfile,
        replacement: &crate::credentials::RefreshedCredentials,
    ) -> Result<CredentialProfile> {
        let repository = self.clone();
        let source = source.clone();
        let replacement = replacement.clone();
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Other("Credential writer is shutting down".into()))?;
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let _permit = permit;
            let profile = repository.refreshed_inner(&source, &replacement).await?;
            if profile.revision != source.revision {
                repository.publish_material(profile.owner()?);
            }
            Ok(profile)
        })
        .await
    }

    async fn refreshed_inner(
        &self,
        source: &CredentialProfile,
        replacement: &crate::credentials::RefreshedCredentials,
    ) -> Result<CredentialProfile> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, &source.id).await?;
        require_current(&mut tx, source, &current).await?;
        let platform =
            require_owner(&mut tx, &current.owner()?, &current.platform_config_id).await?;
        let mut material = current.material()?;
        material.cookies = replacement.cookies.clone();
        if replacement.refresh_token.is_some() {
            material
                .refresh_token
                .clone_from(&replacement.refresh_token);
        }
        if replacement.access_token.is_some() {
            material.access_token.clone_from(&replacement.access_token);
        }
        material.validate(&platform)?;
        if material.cookies == current.cookies
            && material.refresh_token == current.refresh_token
            && material.access_token == current.access_token
        {
            // A provider that found nothing to refresh returns the stored bundle.
            // Confirm the account at its current revision: bound recordings and
            // cooldowns stay attached, while refresh-failure state is cleared.
            let now = crate::database::time::now_ms();
            sqlx::query("INSERT INTO credential_profile_health(profile_id, revision, validity, last_check_at, last_refresh_at) VALUES (?, ?, 'valid', ?, ?) ON CONFLICT(profile_id) DO UPDATE SET validity = 'valid', last_check_at = excluded.last_check_at, last_refresh_at = excluded.last_refresh_at, refresh_failure_count = 0, last_failure_at = NULL, last_notified_failure_count = 0, reason_code = NULL, cooldown_until = CASE WHEN credential_profile_health.revision = excluded.revision THEN credential_profile_health.cooldown_until END, throttle_count = CASE WHEN credential_profile_health.revision = excluded.revision THEN credential_profile_health.throttle_count ELSE 0 END, revision = excluded.revision")
                .bind(&source.id).bind(current.revision).bind(now).bind(now).execute(&mut *tx).await?;
            crate::database::committed_writer::prepare_owned_commit()?;
            tx.commit().await?;
            return Ok(current);
        }
        sqlx::query("UPDATE credential_profiles SET cookies = ?, refresh_token = COALESCE(?, refresh_token), access_token = COALESCE(?, access_token), revision = revision + 1, version = version + 1, updated_at = ? WHERE id = ?")
            .bind(&replacement.cookies).bind(&replacement.refresh_token).bind(&replacement.access_token)
            .bind(crate::database::time::now_ms()).bind(&source.id).execute(&mut *tx).await?;
        let updated = load(&mut tx, &source.id).await?;
        sqlx::query("INSERT OR REPLACE INTO credential_profile_health(profile_id, revision, validity, last_check_at, last_refresh_at) VALUES (?, ?, 'valid', ?, ?)")
            .bind(&source.id).bind(updated.revision).bind(crate::database::time::now_ms()).bind(crate::database::time::now_ms()).execute(&mut *tx).await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.after_commit().await;
        Ok(updated)
    }

    pub async fn health(&self, id: &str) -> Result<Option<CredentialProfileHealth>> {
        Ok(sqlx::query_as::<_, CredentialProfileHealth>("SELECT h.* FROM credential_profile_health h JOIN credential_profiles p ON p.id = h.profile_id AND p.revision = h.revision WHERE h.profile_id = ?")
            .bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn publish_health(
        &self,
        source: &CredentialProfile,
        validity: &str,
        cooldown_until: Option<i64>,
        reason: Option<&str>,
        checked: bool,
    ) -> Result<()> {
        if !matches!(validity, "unknown" | "valid" | "needs_refresh" | "invalid")
            || reason.is_some_and(|value| value.len() > 64)
        {
            return Err(Error::validation("invalid credential health conclusion"));
        }
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, &source.id).await?;
        require_current(&mut tx, source, &current).await?;
        sqlx::query("INSERT INTO credential_profile_health(profile_id, revision, validity, last_check_at, cooldown_until, throttle_count, reason_code) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(profile_id) DO UPDATE SET revision = excluded.revision, validity = excluded.validity, last_check_at = COALESCE(excluded.last_check_at, credential_profile_health.last_check_at), cooldown_until = excluded.cooldown_until, throttle_count = CASE WHEN excluded.cooldown_until IS NULL THEN 0 ELSE credential_profile_health.throttle_count + 1 END, reason_code = excluded.reason_code")
            .bind(&source.id).bind(source.revision).bind(validity).bind(checked.then(crate::database::time::now_ms)).bind(cooldown_until).bind(i64::from(cooldown_until.is_some())).bind(reason).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn references(&self, id: &str) -> Result<Vec<String>> {
        references(&mut *self.pool.acquire().await?, id, None).await
    }

    pub async fn delete(&self, id: &str, expected_version: i64) -> Result<()> {
        let repository = self.clone();
        let id = id.to_owned();
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Other("Credential writer is shutting down".into()))?;
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let _permit = permit;
            let owner = repository.delete_inner(&id, expected_version).await?;
            repository.publish_material(owner);
            Ok(())
        })
        .await
    }

    async fn delete_inner(&self, id: &str, expected_version: i64) -> Result<CredentialOwner> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, id).await?;
        if current.version != expected_version {
            return Err(ProfileError::StaleVersion.into());
        }
        let references = references(&mut tx, id, None).await?;
        if !references.is_empty() {
            return Err(ProfileError::Referenced(references).into());
        }
        sqlx::query("DELETE FROM credential_profiles WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.after_commit().await;
        Ok(current.owner()?)
    }
}

pub(crate) async fn load(connection: &mut SqliteConnection, id: &str) -> Result<CredentialProfile> {
    sqlx::query_as::<_, CredentialProfile>("SELECT * FROM credential_profiles WHERE id = ?")
        .bind(id)
        .fetch_optional(connection)
        .await?
        .ok_or_else(|| Error::not_found("CredentialProfile", id))
}

fn validate_label(label: &str) -> Result<&str> {
    let label = label.trim();
    if !(1..=128).contains(&label.chars().count()) {
        return Err(ProfileError::InvalidMaterial("label must contain 1..128 characters").into());
    }
    Ok(label)
}

/// A missing or retired owner is not found; an owner that exists but cannot
/// hold this platform's profiles is invalid.
pub(crate) async fn require_owner(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
) -> Result<String> {
    let platform: String =
        sqlx::query_scalar("SELECT platform_name FROM platform_config WHERE id = ?")
            .bind(platform_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| Error::not_found("Platform", platform_id))?;
    match owner {
        CredentialOwner::Platform {
            platform_id: owner_platform,
        } => {
            if owner_platform != platform_id {
                return Err(ProfileError::InvalidOwner.into());
            }
        }
        CredentialOwner::Template { template_id } => {
            let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM template_config WHERE id = ? AND NOT EXISTS(SELECT 1 FROM retirement_config_deletions WHERE kind = 'template' AND config_id = template_config.id))")
                .bind(template_id).fetch_one(&mut *connection).await?;
            if !live {
                return Err(Error::not_found("Template", template_id));
            }
        }
        CredentialOwner::Streamer { streamer_id } => {
            let streamer_platform: Option<String> = sqlx::query_scalar(
                "SELECT platform_config_id FROM streamers WHERE id = ? AND deleted_at IS NULL",
            )
            .bind(streamer_id)
            .fetch_optional(&mut *connection)
            .await?;
            match streamer_platform {
                None => return Err(Error::not_found("Streamer", streamer_id)),
                Some(streamer_platform) if streamer_platform != platform_id => {
                    return Err(ProfileError::InvalidOwner.into());
                }
                Some(_) => {}
            }
        }
    }
    Ok(platform)
}

pub(crate) async fn create_in(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
    label: &str,
    enabled: bool,
    material: &CredentialMaterial,
) -> Result<CredentialProfile> {
    let platform = require_owner(connection, owner, platform_id).await?;
    material.validate(&platform)?;
    let label = validate_label(label)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::database::time::now_ms();
    let template_id = match owner {
        CredentialOwner::Template { template_id } => Some(template_id),
        _ => None,
    };
    let streamer_id = match owner {
        CredentialOwner::Streamer { streamer_id } => Some(streamer_id),
        _ => None,
    };
    sqlx::query("INSERT INTO credential_profiles(id, platform_config_id, owner_kind, template_id, streamer_id, label, enabled, cookies, refresh_token, access_token, reauth_config, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&id).bind(platform_id).bind(owner.kind()).bind(template_id).bind(streamer_id).bind(label).bind(enabled).bind(&material.cookies).bind(&material.refresh_token).bind(&material.access_token)
        .bind(material.reauth_config.as_ref().map(serde_json::to_string).transpose()?).bind(now).bind(now).execute(&mut *connection).await?;
    load(connection, &id).await
}

pub(crate) async fn require_current(
    connection: &mut SqliteConnection,
    source: &CredentialProfile,
    current: &CredentialProfile,
) -> Result<()> {
    require_not_retiring(connection, &current.id).await?;
    if !current.enabled
        || source.revision != current.revision
        || source.platform_config_id != current.platform_config_id
        || source.owner()? != current.owner()?
    {
        return Err(ProfileError::SourceChanged.into());
    }
    require_owner(connection, &current.owner()?, &current.platform_config_id).await?;
    Ok(())
}

pub(crate) async fn can_access(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    profile: &CredentialProfile,
) -> Result<bool> {
    let profile_owner = profile.owner()?;
    if profile_owner == *owner
        || matches!(&profile_owner, CredentialOwner::Platform { platform_id } if platform_id == &profile.platform_config_id)
    {
        return Ok(true);
    }
    if let (CredentialOwner::Streamer { streamer_id }, CredentialOwner::Template { template_id }) =
        (owner, profile_owner)
    {
        return Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM streamers WHERE id = ? AND template_config_id = ? AND platform_config_id = ? AND deleted_at IS NULL)")
            .bind(streamer_id).bind(template_id).bind(&profile.platform_config_id).fetch_one(connection).await?);
    }
    Ok(false)
}

pub(crate) async fn validate_policy(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
    selection: &CredentialSelection,
) -> Result<()> {
    selection.validate()?;
    require_owner(connection, owner, platform_id).await?;
    for id in selection.profile_ids() {
        require_not_retiring(connection, id).await?;
        let profile = load(connection, id).await?;
        if profile.platform_config_id != platform_id
            || !can_access(connection, owner, &profile).await?
        {
            return Err(ProfileError::InvalidOwner.into());
        }
        require_owner(connection, &profile.owner()?, platform_id).await?;
    }
    Ok(())
}

pub(crate) async fn require_not_retiring(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<()> {
    let retiring: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM retirement_credential_profiles WHERE profile_id = ?)",
    )
    .bind(id)
    .fetch_one(connection)
    .await?;
    if retiring {
        return Err(ProfileError::SourceChanged.into());
    }
    Ok(())
}

pub(crate) async fn commit_session_binding(
    connection: &mut SqliteConnection,
    session_id: &str,
    streamer_id: &str,
    incoming: &crate::credentials::CredentialBinding,
    allow_rebind: bool,
) -> Result<crate::credentials::CredentialBinding> {
    use crate::credentials::CredentialIdentity;
    let raw: Option<String> = sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = ? AND streamer_id = ? AND end_time IS NULL").bind(session_id).bind(streamer_id).fetch_optional(&mut *connection).await?.ok_or(ProfileError::SourceChanged)?;
    let existing = raw
        .as_deref()
        .map(serde_json::from_str::<crate::credentials::CredentialBinding>)
        .transpose()?;
    if existing.is_none() && incoming.epoch != 0 {
        return Err(ProfileError::SourceChanged.into());
    }
    if let Some(existing) = &existing
        && incoming.epoch != existing.epoch
    {
        return Err(ProfileError::SourceChanged.into());
    }
    let row = sqlx::query_as::<_, crate::database::models::StreamerDbModel>(
        "SELECT * FROM streamers WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(streamer_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(ProfileError::SourceChanged)?;
    let platform = sqlx::query_as::<_, crate::database::models::PlatformConfigDbModel>(
        "SELECT * FROM platform_config WHERE id = ?",
    )
    .bind(&row.platform_config_id)
    .fetch_one(&mut *connection)
    .await?;
    let template = match &row.template_config_id {
        Some(id) => {
            sqlx::query_as::<_, crate::database::models::TemplateConfigDbModel>(
                "SELECT * FROM template_config WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?
        }
        None => None,
    };
    let mut streamer = crate::domain::streamer::Streamer::new(
        &row.name,
        crate::domain::StreamerUrl::new(&row.url)?,
        &row.platform_config_id,
    );
    streamer.id = row.id;
    streamer.template_config_id = row.template_config_id;
    streamer.streamer_specific_config = row
        .streamer_specific_config
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let resolved =
        crate::credentials::resolve_authentication(&streamer, &platform, template.as_ref())?;
    if resolved.policy.as_ref() != Some(&incoming.policy) {
        return Err(ProfileError::SourceChanged.into());
    }
    validate_policy(
        connection,
        &incoming.policy.owner,
        &incoming.policy.platform_id,
        &incoming.policy.selection,
    )
    .await?;
    if let CredentialIdentity::Profile { profile_id } = &incoming.identity {
        let profile = load(connection, profile_id).await?;
        require_current(connection, &profile, &profile).await?;
        if profile.revision as u64 != incoming.revision
            || !incoming
                .policy
                .selection
                .profile_ids()
                .contains(&profile_id.as_str())
        {
            return Err(ProfileError::SourceChanged.into());
        }
    } else if !matches!(incoming.identity, CredentialIdentity::Anonymous)
        || incoming.revision != 0
        || !matches!(
            incoming.policy.selection,
            CredentialSelection::None | CredentialSelection::Inherit
        )
    {
        return Err(ProfileError::SourceChanged.into());
    }
    if let Some(existing) = &existing {
        if !allow_rebind {
            if existing.identity != incoming.identity {
                return Err(ProfileError::SourceChanged.into());
            }
            return Ok(existing.clone());
        }
        if existing.identity == incoming.identity
            && existing.revision == incoming.revision
            && existing.policy.generation == incoming.policy.generation
        {
            return Ok(existing.clone());
        }
    }
    let mut binding = incoming.clone();
    binding.epoch = match existing {
        Some(existing) => existing
            .epoch
            .checked_add(1)
            .ok_or_else(|| Error::validation("credential binding epoch exhausted"))?,
        None => 1,
    };
    sqlx::query(
        "UPDATE live_sessions SET credential_binding = ? WHERE id = ? AND end_time IS NULL",
    )
    .bind(serde_json::to_string(&binding)?)
    .bind(session_id)
    .execute(connection)
    .await?;
    Ok(binding)
}

/// Enumerate only selection fields, never arbitrary strings or secret JSON values.
pub(crate) async fn references(
    connection: &mut SqliteConnection,
    id: &str,
    retiring_owner: Option<&CredentialOwner>,
) -> Result<Vec<String>> {
    let mut references = Vec::new();
    let platforms: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, credential_selection FROM platform_config")
            .fetch_all(&mut *connection)
            .await?;
    for (owner_id, policy) in platforms {
        let owner = CredentialOwner::Platform {
            platform_id: owner_id,
        };
        if retiring_owner != Some(&owner)
            && policy
                .as_deref()
                .map(policy_contains)
                .transpose()?
                .is_some_and(|ids| ids.contains(&id.to_string()))
        {
            references.push(format!("platform:{}", owner.id()));
        }
    }
    let templates: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, platform_overrides FROM template_config")
            .fetch_all(&mut *connection)
            .await?;
    for (owner_id, overrides) in templates {
        let owner = CredentialOwner::Template {
            template_id: owner_id,
        };
        if retiring_owner == Some(&owner) {
            continue;
        }
        let value: serde_json::Value = overrides
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        if let Some(fields) = value.as_object() {
            for (platform, config) in fields {
                if let Some(policy) = config.get("credential_selection")
                    && CredentialSelection::from_value(policy.clone())?
                        .profile_ids()
                        .contains(&id)
                {
                    references.push(format!("template:{}:{platform}", owner.id()));
                }
            }
        }
    }
    let streamers: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, streamer_specific_config FROM streamers")
            .fetch_all(&mut *connection)
            .await?;
    for (owner_id, config) in streamers {
        let owner = CredentialOwner::Streamer {
            streamer_id: owner_id,
        };
        if retiring_owner == Some(&owner) {
            continue;
        }
        let value: serde_json::Value = config
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        if let Some(policy) = value.get("credential_selection")
            && CredentialSelection::from_value(policy.clone())?
                .profile_ids()
                .contains(&id)
        {
            references.push(format!("streamer:{}", owner.id()));
        }
    }
    let sessions: Vec<(String, String)> = sqlx::query_as("SELECT id, credential_binding FROM live_sessions WHERE end_time IS NULL AND credential_binding IS NOT NULL").fetch_all(&mut *connection).await?;
    for (session_id, raw) in sessions {
        let binding: crate::credentials::CredentialBinding = serde_json::from_str(&raw)?;
        if matches!(binding.identity, crate::credentials::CredentialIdentity::Profile { profile_id } if profile_id == id)
        {
            references.push(format!("session:{session_id}"));
        }
    }
    Ok(references)
}

fn policy_contains(raw: &str) -> Result<Vec<String>> {
    Ok(CredentialSelection::from_value(serde_json::from_str(raw)?)?
        .profile_ids()
        .into_iter()
        .map(str::to_string)
        .collect())
}

/// A missing, moved or foreign profile makes the selection that names it
/// unusable; other errors are not about the reference.
fn inaccessible(error: &Error) -> bool {
    matches!(
        error,
        Error::NotFound { .. } | Error::CredentialProfile(ProfileError::InvalidOwner)
    )
}

/// Every surviving selection must still be usable by its owner. A write that
/// breaks one reports all the referring configs so the caller can update them.
pub(crate) async fn validate_graph(connection: &mut SqliteConnection) -> Result<()> {
    let mut broken: Vec<String> = sqlx::query_scalar("SELECT DISTINCT 'streamer:' || s.id FROM credential_profiles p JOIN streamers s ON s.id = p.streamer_id WHERE p.platform_config_id != s.platform_config_id AND s.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM retirement_credential_profiles r WHERE r.profile_id = p.id) ORDER BY s.id").fetch_all(&mut *connection).await?;
    let platforms: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, platform_name, credential_selection FROM platform_config")
            .fetch_all(&mut *connection)
            .await?;
    for (id, _, policy) in &platforms {
        if let Some(raw) = policy {
            let result = validate_policy(
                connection,
                &CredentialOwner::Platform {
                    platform_id: id.clone(),
                },
                id,
                &CredentialSelection::from_value(serde_json::from_str(raw)?)?,
            )
            .await;
            match result {
                Err(error) if inaccessible(&error) => broken.push(format!("platform:{id}")),
                result => result?,
            }
        }
    }
    let templates: Vec<(String, Option<String>)> = sqlx::query_as("SELECT id, platform_overrides FROM template_config WHERE NOT EXISTS(SELECT 1 FROM retirement_config_deletions WHERE kind = 'template' AND config_id = template_config.id)").fetch_all(&mut *connection).await?;
    for (id, raw) in templates {
        let value: serde_json::Value = raw
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default();
        if let Some(fields) = value.as_object() {
            for (name, config) in fields {
                if let Some(policy) = config.get("credential_selection") {
                    let platform_id = platforms.iter().find(|(_, stored_name, _)| stored_name == name).map(|(id, _, _)| id)
                        .ok_or_else(|| Error::validation(format!("managed template override must use a canonical platform name; available names: {}", platforms.iter().map(|(_, name, _)| name.as_str()).collect::<Vec<_>>().join(", "))))?;
                    let result = validate_policy(
                        connection,
                        &CredentialOwner::Template {
                            template_id: id.clone(),
                        },
                        platform_id,
                        &CredentialSelection::from_value(policy.clone())?,
                    )
                    .await;
                    match result {
                        Err(error) if inaccessible(&error) => {
                            broken.push(format!("template:{id}:{name}"))
                        }
                        result => result?,
                    }
                }
            }
        }
    }
    let streamers: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT id, platform_config_id, streamer_specific_config FROM streamers WHERE deleted_at IS NULL").fetch_all(&mut *connection).await?;
    for (id, platform, raw) in streamers {
        let value: serde_json::Value = raw
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default();
        if let Some(policy) = value.get("credential_selection") {
            let reference = format!("streamer:{id}");
            let result = validate_policy(
                connection,
                &CredentialOwner::Streamer { streamer_id: id },
                &platform,
                &CredentialSelection::from_value(policy.clone())?,
            )
            .await;
            match result {
                Err(error) if inaccessible(&error) => {
                    if !broken.contains(&reference) {
                        broken.push(reference);
                    }
                }
                result => result?,
            }
        }
    }
    if broken.is_empty() {
        Ok(())
    } else {
        Err(ProfileError::InaccessibleReferences(broken).into())
    }
}

fn auth_fields(raw: Option<&str>) -> serde_json::Value {
    let value: serde_json::Value = raw
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    fn project(value: &serde_json::Value) -> serde_json::Value {
        let mut auth = serde_json::Map::new();
        for key in [
            "cookies",
            "reauth_config",
            "refresh_token",
            "access_token",
            "oauth_token",
            "ttwid",
            "device_id",
            "username",
            "password",
            "session_cookies",
        ] {
            if let Some(value) = value.get(key) {
                auth.insert(key.to_string(), value.clone());
            }
        }
        for key in ["platform_specific_config", "platform_extras"] {
            if let Some(value) = value.get(key) {
                let projected = project(value);
                if projected
                    .as_object()
                    .is_some_and(|fields| !fields.is_empty())
                {
                    auth.insert(key.to_string(), projected);
                }
            }
        }
        serde_json::Value::Object(auth)
    }

    project(&value)
}

fn stored_policy_json(raw: Option<&str>) -> Result<serde_json::Value> {
    match raw.filter(|raw| !raw.is_empty()) {
        Some(raw) => Ok(serde_json::from_str(raw)?),
        None => Ok(serde_json::Value::Null),
    }
}

fn object_for_policy(
    value: &mut serde_json::Value,
) -> Result<&mut serde_json::Map<String, serde_json::Value>> {
    if value.is_null() {
        *value = serde_json::json!({});
    }
    value.as_object_mut().ok_or_else(|| {
        Error::validation("A retained credential policy requires an object configuration")
    })
}

/// An omitted selection means unchanged; `inherit` is the explicit reset.
pub(crate) fn preserve_streamer_policy(
    old: &crate::database::models::StreamerDbModel,
    new: &mut crate::database::models::StreamerDbModel,
) -> Result<()> {
    let Ok(old) = stored_policy_json(old.streamer_specific_config.as_deref()) else {
        // Legacy opaque documents cannot contain a stored managed selection.
        return Ok(());
    };
    if let Some(policy) = old.get("credential_selection") {
        let mut replacement = stored_policy_json(new.streamer_specific_config.as_deref())?;
        object_for_policy(&mut replacement)?
            .entry("credential_selection")
            .or_insert_with(|| policy.clone());
        new.streamer_specific_config = Some(replacement.to_string());
    }
    Ok(())
}

/// Keeps each platform override's selection that the replacement omits,
/// including when the whole override map or that platform's entry is omitted.
pub(crate) fn preserve_template_policies(
    old: &crate::database::models::TemplateConfigDbModel,
    new: &mut crate::database::models::TemplateConfigDbModel,
) -> Result<()> {
    let Ok(old) = stored_policy_json(old.platform_overrides.as_deref()) else {
        return Ok(());
    };
    let Some(entries) = old.as_object() else {
        return Ok(());
    };
    let policies: Vec<_> = entries
        .iter()
        .filter_map(|(platform, value)| {
            value
                .get("credential_selection")
                .map(|policy| (platform, policy))
        })
        .collect();
    if policies.is_empty() {
        return Ok(());
    }
    let mut replacement = stored_policy_json(new.platform_overrides.as_deref())?;
    let entries = object_for_policy(&mut replacement)?;
    for (platform, policy) in policies {
        let entry = entries
            .entry(platform.clone())
            .or_insert(serde_json::Value::Null);
        object_for_policy(entry)?
            .entry("credential_selection")
            .or_insert_with(|| policy.clone());
    }
    new.platform_overrides = Some(replacement.to_string());
    Ok(())
}

pub(crate) fn guard_platform_legacy(
    old: &crate::database::models::PlatformConfigDbModel,
    new: &crate::database::models::PlatformConfigDbModel,
) -> Result<()> {
    if old.credential_selection.is_some()
        && (new.credential_selection.is_none()
            || old.cookies != new.cookies
            || auth_fields(old.platform_specific_config.as_deref())
                != auth_fields(new.platform_specific_config.as_deref()))
    {
        return Err(ProfileError::SourceChanged.into());
    }
    Ok(())
}

pub(crate) fn guard_template_legacy(
    old: &crate::database::models::TemplateConfigDbModel,
    new: &crate::database::models::TemplateConfigDbModel,
) -> Result<()> {
    let old_json: serde_json::Value = old
        .platform_overrides
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let new_json: serde_json::Value = new
        .platform_overrides
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    if let Some(fields) = old_json.as_object() {
        for (platform, config) in fields {
            if config.get("credential_selection").is_some() {
                let replacement = &new_json[platform];
                if replacement.get("credential_selection").is_none()
                    || auth_fields(Some(&config.to_string()))
                        != auth_fields(Some(&replacement.to_string()))
                {
                    return Err(ProfileError::SourceChanged.into());
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn guard_template_legacy_in(
    connection: &mut SqliteConnection,
    old: &crate::database::models::TemplateConfigDbModel,
    new: &crate::database::models::TemplateConfigDbModel,
) -> Result<()> {
    guard_template_legacy(old, new)?;
    if old.cookies != new.cookies {
        let overrides: serde_json::Value = old
            .platform_overrides
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        let platforms: Vec<String> =
            sqlx::query_scalar("SELECT platform_name FROM platform_config")
                .fetch_all(connection)
                .await?;
        if !platforms.is_empty()
            && platforms.iter().all(|name| {
                overrides
                    .get(name)
                    .and_then(|entry| entry.get("credential_selection"))
                    .is_some()
            })
        {
            return Err(ProfileError::SourceChanged.into());
        }
    }
    Ok(())
}

pub(crate) fn guard_streamer_legacy(
    old: &crate::database::models::StreamerDbModel,
    new: &crate::database::models::StreamerDbModel,
) -> Result<()> {
    let old_json: serde_json::Value = old
        .streamer_specific_config
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let new_json: serde_json::Value = new
        .streamer_specific_config
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    if old_json.get("credential_selection").is_some()
        && (new_json.get("credential_selection").is_none()
            || auth_fields(old.streamer_specific_config.as_deref())
                != auth_fields(new.streamer_specific_config.as_deref()))
    {
        return Err(ProfileError::SourceChanged.into());
    }
    Ok(())
}

/// Caller owns BEGIN IMMEDIATE; removing a retiring owner's policies is safe only
/// after every surviving reference and active binding has been checked.
pub(crate) async fn delete_owner_profiles(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
) -> Result<()> {
    if let CredentialOwner::Platform { platform_id } = owner {
        let mut dependencies: Vec<String> = sqlx::query_scalar("SELECT 'streamer:' || id FROM streamers WHERE platform_config_id = ? UNION ALL SELECT 'profile:' || id FROM credential_profiles WHERE platform_config_id = ? AND owner_kind != 'platform'").bind(platform_id).bind(platform_id).fetch_all(&mut *connection).await?;
        let policies: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, platform_overrides FROM template_config")
                .fetch_all(&mut *connection)
                .await?;
        let name: Option<String> =
            sqlx::query_scalar("SELECT platform_name FROM platform_config WHERE id = ?")
                .bind(platform_id)
                .fetch_optional(&mut *connection)
                .await?;
        if let Some(name) = name {
            for (id, raw) in policies {
                let value: serde_json::Value = raw
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_default();
                if value
                    .get(&name)
                    .and_then(|entry| entry.get("credential_selection"))
                    .is_some()
                {
                    dependencies.push(format!("template:{id}:{name}"));
                }
            }
        }
        if !dependencies.is_empty() {
            return Err(ProfileError::Referenced(dependencies).into());
        }
    }
    let profiles = sqlx::query_as::<_, CredentialProfile>("SELECT * FROM credential_profiles WHERE (owner_kind = 'platform' AND platform_config_id = ? AND ? = 'platform') OR (template_id = ? AND ? = 'template') OR (streamer_id = ? AND ? = 'streamer')")
        .bind(owner.id()).bind(owner.kind()).bind(owner.id()).bind(owner.kind()).bind(owner.id()).bind(owner.kind()).fetch_all(&mut *connection).await?;
    for profile in &profiles {
        let references = references(connection, &profile.id, Some(owner)).await?;
        if !references.is_empty() {
            return Err(ProfileError::Referenced(references).into());
        }
    }
    for profile in profiles {
        sqlx::query("DELETE FROM credential_profiles WHERE id = ?")
            .bind(profile.id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
