//! Transactional account storage.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use sqlx::{SqliteConnection, SqlitePool};

use crate::credentials::{
    CredentialMaterial, CredentialOwner, CredentialProfile, CredentialProfileHealth,
    CredentialSelection, CredentialValidity, HealthReason, ProfileError, ProfileReferences,
    RecordingReference,
};
use crate::database::begin_immediate;
use crate::proxies::ProxyRoute;
use crate::{Error, Result};

type CredentialPublication = Arc<dyn Fn(CredentialOwner) + Send + Sync>;

/// What an account edit changes; each field left `None` keeps the stored
/// value.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProfileEdit<'a> {
    pub label: Option<&'a str>,
    pub enabled: Option<bool>,
    /// Replaces the whole material bundle.
    pub replacement: Option<&'a CredentialMaterial>,
    /// Replaces the account's own route.
    pub route: Option<&'a ProxyRoute>,
    /// Replaces a Streamlink account's sites. Sites are not material: they
    /// change which streamers use the account, not the account, so they keep
    /// its revision.
    pub sites: Option<&'a [String]>,
}

#[derive(Clone)]
pub struct CredentialProfileRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
    supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
    publication: Arc<OnceLock<CredentialPublication>>,
    material_publication: Arc<OnceLock<CredentialPublication>>,
    admission: Arc<tokio::sync::Semaphore>,
    /// Bumped after any write that can change which accounts need the user.
    attention: Arc<tokio::sync::watch::Sender<u64>>,
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
        if changed {
            self.attention_changed();
        }
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
            attention: Arc::new(tokio::sync::watch::Sender::new(0)),
            #[cfg(test)]
            commit_gate: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Wakes after writes that can change [`Self::needing_attention`]: health,
    /// material, the enabled state and deletion. Changes in between coalesce.
    pub fn attention_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.attention.subscribe()
    }

    fn attention_changed(&self) {
        self.attention.send_modify(|generation| {
            *generation = generation.wrapping_add(1);
        });
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

    fn publish_owner(&self, owner: CredentialOwner) {
        if let Some(publish) = self.publication.get() {
            publish(owner);
        }
    }

    pub(crate) fn bind_material_publication(&self, publication: CredentialPublication) {
        self.material_publication.get_or_init(|| publication);
    }

    pub(crate) fn publish_material(&self, owner: CredentialOwner) {
        self.attention_changed();
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
        Ok(CredentialScope::for_owner(owner, name))
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
        self.attention_changed();
        Ok((count, notify))
    }

    pub async fn get(&self, id: &str) -> Result<CredentialProfile> {
        load(&mut *self.pool.acquire().await?, id).await
    }

    #[cfg(test)]
    pub(crate) fn write_pool_for_tests(&self) -> SqlitePool {
        self.write_pool.clone()
    }

    /// The route requests made with `profile` take: its own route, else the
    /// `operation`'s. Without an operation (checks, refreshes and sign-in of
    /// the account itself) an account that inherits follows its platform,
    /// then the global route.
    pub async fn account_route(
        &self,
        profile: &CredentialProfile,
        operation: Option<&crate::proxies::ResolvedRoute>,
    ) -> Result<crate::proxies::ResolvedRoute> {
        super::proxies::resolve_account(
            &mut *self.pool.acquire().await?,
            &profile.platform_config_id,
            &profile.route()?,
            operation,
            crate::proxies::SystemProxy::current(),
        )
        .await
    }

    /// Every profile of the platform, after checking the selecting scope exists
    /// on it.
    pub async fn accessible(
        &self,
        owner: &CredentialOwner,
        platform_id: &str,
    ) -> Result<Vec<CredentialProfile>> {
        let mut connection = self.pool.acquire().await?;
        require_owner(&mut connection, owner, platform_id).await?;
        Ok(sqlx::query_as::<_, CredentialProfile>("SELECT * FROM credential_profiles WHERE platform_config_id = ? ORDER BY created_at, id")
            .bind(platform_id).fetch_all(&mut *connection).await?)
    }

    pub async fn create(
        &self,
        platform_id: &str,
        label: &str,
        enabled: bool,
        material: &CredentialMaterial,
        route: &ProxyRoute,
    ) -> Result<CredentialProfile> {
        self.create_with_sites(platform_id, label, enabled, material, route, &[])
            .await
    }

    /// [`Self::create`] for an account that names the `sites` it is for.
    pub async fn create_with_sites(
        &self,
        platform_id: &str,
        label: &str,
        enabled: bool,
        material: &CredentialMaterial,
        route: &ProxyRoute,
        sites: &[String],
    ) -> Result<CredentialProfile> {
        let repository = self.clone();
        let platform_id = platform_id.to_owned();
        let label = label.to_owned();
        let material = material.clone();
        let route = route.clone();
        let sites = sites.to_vec();
        self.run_owned(async move {
            let profile = repository
                .create_inner(&platform_id, &label, enabled, &material, &route, &sites)
                .await?;
            repository.publish_sites(&profile, !sites.is_empty());
            Ok(profile)
        })
        .await
    }

    /// Publishes a committed account write. Sites decide which account a
    /// Streamlink streamer resolves to, so a change of sites republishes the
    /// platform's configuration as well as the account.
    fn publish_sites(&self, profile: &CredentialProfile, sites_changed: bool) {
        if sites_changed {
            self.publish_owner(profile.owner());
        }
        self.publish_material(profile.owner());
    }

    /// Admits writes one at a time and runs each as an owned operation, so it
    /// commits and publishes even if the caller is cancelled.
    async fn run_owned<T, F>(&self, write: F) -> Result<T>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Other("Credential writer is shutting down".into()))?;
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let _permit = permit;
            write.await
        })
        .await
    }

    async fn create_inner(
        &self,
        platform_id: &str,
        label: &str,
        enabled: bool,
        material: &CredentialMaterial,
        route: &ProxyRoute,
        sites: &[String],
    ) -> Result<CredentialProfile> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let profile = create_in(&mut tx, platform_id, label, enabled, material, route).await?;
        set_sites(&mut tx, &profile.id, sites).await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.after_commit().await;
        Ok(profile)
    }

    /// `route` replaces the account's own route when present.
    pub async fn update(
        &self,
        id: &str,
        expected_version: i64,
        label: Option<&str>,
        enabled: Option<bool>,
        replacement: Option<&CredentialMaterial>,
        route: Option<&ProxyRoute>,
    ) -> Result<CredentialProfile> {
        self.edit(
            id,
            expected_version,
            ProfileEdit {
                label,
                enabled,
                replacement,
                route,
                sites: None,
            },
        )
        .await
    }

    /// Applies `edit` to the account at `expected_version`.
    pub async fn edit(
        &self,
        id: &str,
        expected_version: i64,
        edit: ProfileEdit<'_>,
    ) -> Result<CredentialProfile> {
        let repository = self.clone();
        let id = id.to_owned();
        let label = edit.label.map(str::to_owned);
        let enabled = edit.enabled;
        let replacement = edit.replacement.cloned();
        let route = edit.route.cloned();
        let sites = edit.sites.map(<[String]>::to_vec);
        self.run_owned(async move {
            let mut tx = begin_immediate(&repository.write_pool).await?;
            let profile = update_in(
                &mut tx,
                &id,
                expected_version,
                label.as_deref(),
                enabled,
                replacement.as_ref(),
                route.as_ref(),
            )
            .await?;
            let sites_changed = match &sites {
                Some(sites) => set_sites(&mut tx, &id, sites).await?,
                None => false,
            };
            crate::database::committed_writer::prepare_owned_commit()?;
            tx.commit().await?;
            repository.after_commit().await;
            repository.publish_sites(&profile, sites_changed);
            Ok(profile)
        })
        .await
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
        self.run_owned(async move {
            let profile = repository.refreshed_inner(&source, &replacement).await?;
            if profile.revision != source.revision {
                repository.publish_material(profile.owner());
            } else {
                // Confirming the stored bundle still clears refresh failures.
                repository.attention_changed();
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
        let platform = require_platform(&mut tx, &current.platform_config_id).await?;
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
            // Confirm the account at its current revision: bound recordings stay
            // attached, while refresh-failure state is cleared.
            let now = crate::database::time::now_ms();
            sqlx::query("INSERT INTO credential_profile_health(profile_id, revision, validity, last_check_at, last_refresh_at) VALUES (?, ?, 'valid', ?, ?) ON CONFLICT(profile_id) DO UPDATE SET validity = 'valid', last_check_at = excluded.last_check_at, last_refresh_at = excluded.last_refresh_at, refresh_failure_count = 0, last_failure_at = NULL, last_notified_failure_count = 0, reason_code = NULL, revision = excluded.revision")
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
        validity: CredentialValidity,
        reason: Option<HealthReason>,
        checked: bool,
    ) -> Result<()> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, &source.id).await?;
        require_current(&mut tx, source, &current).await?;
        sqlx::query("INSERT INTO credential_profile_health(profile_id, revision, validity, last_check_at, reason_code) VALUES (?, ?, ?, ?, ?) ON CONFLICT(profile_id) DO UPDATE SET revision = excluded.revision, validity = excluded.validity, last_check_at = COALESCE(excluded.last_check_at, credential_profile_health.last_check_at), reason_code = excluded.reason_code")
            .bind(&source.id).bind(source.revision).bind(validity).bind(checked.then(crate::database::time::now_ms)).bind(reason).execute(&mut *tx).await?;
        tx.commit().await?;
        self.attention_changed();
        Ok(())
    }

    /// Enabled accounts that need the user, ordered by platform and label:
    /// those only a new login can fix, and rejected ones whose refresh failed
    /// [`crate::credentials::ATTENTION_REFRESH_FAILURES`] times in a row.
    /// Disabling is the user's own choice, so disabled accounts are left out,
    /// as are accounts an import is retiring.
    pub async fn needing_attention(&self) -> Result<Vec<crate::credentials::CredentialAttention>> {
        #[derive(sqlx::FromRow)]
        struct Row {
            #[sqlx(flatten)]
            profile: CredentialProfile,
            validity: CredentialValidity,
            last_check_at: Option<i64>,
            last_refresh_at: Option<i64>,
            refresh_failure_count: i64,
            last_failure_at: Option<i64>,
            last_notified_failure_count: i64,
            reason_code: Option<HealthReason>,
            platform_name: String,
        }
        let rows = sqlx::query_as::<_, Row>("SELECT p.*, h.validity, h.last_check_at, h.last_refresh_at, h.refresh_failure_count, h.last_failure_at, h.last_notified_failure_count, h.reason_code, c.platform_name FROM credential_profiles p JOIN credential_profile_health h ON h.profile_id = p.id AND h.revision = p.revision JOIN platform_config c ON c.id = p.platform_config_id WHERE p.enabled = 1 AND NOT EXISTS (SELECT 1 FROM retirement_credential_profiles r WHERE r.profile_id = p.id) AND (h.validity = 'invalid' OR (h.validity = 'needs_refresh' AND h.refresh_failure_count >= ?)) ORDER BY c.platform_name, p.label, p.id")
            .bind(crate::credentials::ATTENTION_REFRESH_FAILURES)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let health = CredentialProfileHealth {
                    profile_id: row.profile.id.clone(),
                    revision: row.profile.revision,
                    validity: row.validity,
                    last_check_at: row.last_check_at,
                    last_refresh_at: row.last_refresh_at,
                    refresh_failure_count: row.refresh_failure_count,
                    last_failure_at: row.last_failure_at,
                    last_notified_failure_count: row.last_notified_failure_count,
                    reason_code: row.reason_code,
                };
                let reason = crate::credentials::AttentionReason::for_health(&health)?;
                Some(crate::credentials::CredentialAttention {
                    profile: row.profile.summary(),
                    platform_name: row.platform_name,
                    reason,
                    health,
                })
            })
            .collect())
    }

    /// Current health of several profiles at once; profiles without health
    /// at their current revision have no entry.
    pub async fn health_of(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, CredentialProfileHealth>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, CredentialProfileHealth>("SELECT h.* FROM credential_profile_health h JOIN credential_profiles p ON p.id = h.profile_id AND p.revision = h.revision WHERE h.profile_id IN (SELECT value FROM json_each(?))")
            .bind(serde_json::to_string(ids)?).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|health| (health.profile_id.clone(), health))
            .collect())
    }

    /// The sites of several profiles at once, each list sorted; profiles
    /// without sites have no entry.
    pub async fn sites_of(&self, ids: &[String]) -> Result<HashMap<String, Vec<String>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT profile_id, site FROM credential_profile_sites WHERE profile_id IN (SELECT value FROM json_each(?)) ORDER BY profile_id, site")
            .bind(serde_json::to_string(ids)?)
            .fetch_all(&self.pool)
            .await?;
        let mut sites: HashMap<String, Vec<String>> = HashMap::new();
        for (profile_id, site) in rows {
            sites.entry(profile_id).or_default().push(site);
        }
        Ok(sites)
    }

    /// The host of a Streamlink streamer's URL and the accounts whose sites
    /// cover it, most specific site first. `None` for a streamer on another
    /// platform.
    pub async fn streamer_site(
        &self,
        streamer_id: &str,
    ) -> Result<Option<crate::credentials::StreamerSite>> {
        let mut connection = self.pool.acquire().await?;
        let streamer: Option<(String, String, String)> = sqlx::query_as("SELECT s.url, s.platform_config_id, p.platform_name FROM streamers s JOIN platform_config p ON p.id = s.platform_config_id WHERE s.id = ? AND s.deleted_at IS NULL")
            .bind(streamer_id)
            .fetch_optional(&mut *connection)
            .await?;
        let Some((url, platform_id, platform_name)) = streamer else {
            return Err(Error::not_found("Streamer", streamer_id));
        };
        if !crate::domain::is_streamlink_platform(&platform_name) {
            return Ok(None);
        }
        let accounts = site_accounts(&mut connection, &platform_id, &url).await?;
        Ok(Some(crate::credentials::StreamerSite {
            host: crate::credentials::site_host(&url).unwrap_or_default(),
            site: accounts.first().map(|(site, _)| site.clone()),
            accounts: accounts.into_iter().map(|(_, id)| id).collect(),
        }))
    }

    pub async fn references(&self, id: &str) -> Result<ProfileReferences> {
        references(&mut *self.pool.acquire().await?, id).await
    }

    /// The references of several profiles, read once for all of them.
    pub async fn references_of(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, ProfileReferences>> {
        references_of(&mut *self.pool.acquire().await?, ids).await
    }

    /// Stores `at` as the profile's last use unless the stored time is newer
    /// than `at - window`, so frequent use writes at most once per window.
    /// Returns whether the row changed. Neither the revision nor the version
    /// changes: a use is not an edit.
    pub async fn record_use(&self, id: &str, at: i64, window: std::time::Duration) -> Result<bool> {
        let window = i64::try_from(window.as_millis()).unwrap_or(i64::MAX);
        Ok(sqlx::query("UPDATE credential_profiles SET last_used_at = ?1 WHERE id = ?2 AND (last_used_at IS NULL OR last_used_at <= ?1 - ?3)")
            .bind(at)
            .bind(id)
            .bind(window)
            .execute(&self.write_pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// The selections a scope resolves through. A streamer reads its own, its
    /// template's and its platform's.
    pub async fn selection_layers(
        &self,
        owner: &CredentialOwner,
        platform_id: &str,
    ) -> Result<Vec<super::credential_selections::StoredSelection>> {
        let mut connection = self.pool.acquire().await?;
        match owner {
            CredentialOwner::Streamer { streamer_id } => {
                let template_id: Option<Option<String>> = sqlx::query_scalar(
                    "SELECT template_config_id FROM streamers WHERE id = ? AND deleted_at IS NULL",
                )
                .bind(streamer_id)
                .fetch_optional(&mut *connection)
                .await?;
                let template_id =
                    template_id.ok_or_else(|| Error::not_found("Streamer", streamer_id))?;
                super::credential_selections::load_for_streamer(
                    &mut connection,
                    streamer_id,
                    platform_id,
                    template_id.as_deref(),
                )
                .await
            }
            owner => {
                super::credential_selections::load_for_owner(&mut connection, owner, platform_id)
                    .await
            }
        }
    }

    pub async fn delete(&self, id: &str, expected_version: i64) -> Result<()> {
        let repository = self.clone();
        let id = id.to_owned();
        self.run_owned(async move {
            let (owner, had_sites) = repository.delete_inner(&id, expected_version).await?;
            if had_sites {
                repository.publish_owner(owner.clone());
            }
            repository.publish_material(owner);
            Ok(())
        })
        .await
    }

    /// Returns the deleted account's owner and whether it named any sites.
    async fn delete_inner(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<(CredentialOwner, bool)> {
        let mut tx = begin_immediate(&self.write_pool).await?;
        let current = load(&mut tx, id).await?;
        if current.version != expected_version {
            return Err(ProfileError::StaleVersion.into());
        }
        let references = references(&mut tx, id).await?;
        if !references.is_empty() {
            return Err(ProfileError::Referenced(references).into());
        }
        let had_sites = !sites_in(&mut tx, id).await?.is_empty();
        // SQLite refuses to delete a selected profile too; checking first, in
        // the same transaction, lets the refusal name the selecting scopes.
        sqlx::query("DELETE FROM credential_profiles WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.after_commit().await;
        Ok((current.owner(), had_sites))
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

/// Caller owns BEGIN IMMEDIATE. An edit, including a QR login into an existing
/// profile, needs the version the caller read and is refused while the profile
/// is retiring. Changed material, route or enablement starts a new revision and
/// drops the profile's health: recordings bound to the old revision renew, and
/// a check through the new route decides validity again.
pub(crate) async fn update_in(
    connection: &mut SqliteConnection,
    id: &str,
    expected_version: i64,
    label: Option<&str>,
    enabled: Option<bool>,
    replacement: Option<&CredentialMaterial>,
    route: Option<&ProxyRoute>,
) -> Result<CredentialProfile> {
    let current = load(&mut *connection, id).await?;
    if current.version != expected_version {
        return Err(ProfileError::StaleVersion.into());
    }
    require_not_retiring(&mut *connection, id).await?;
    let platform = require_platform(&mut *connection, &current.platform_config_id).await?;
    let label = validate_label(label.unwrap_or(&current.label))?;
    if let Some(material) = replacement {
        material.validate(&platform)?;
    }
    let material = match replacement {
        Some(material) => material.clone(),
        None => current.material()?,
    };
    let route_changed = match route {
        Some(route) if *route != current.route()? => {
            super::proxies::set_route(
                connection,
                &super::proxies::RouteOwner::Account(id.to_owned()),
                route,
            )
            .await?;
            true
        }
        _ => false,
    };
    let enabled = enabled.unwrap_or(current.enabled);
    let changed = replacement.is_some() || route_changed || enabled != current.enabled;
    sqlx::query("UPDATE credential_profiles SET label = ?, enabled = ?, cookies = ?, refresh_token = ?, access_token = ?, reauth_config = ?, revision = revision + ?, version = version + 1, updated_at = ? WHERE id = ? AND version = ?")
        .bind(label).bind(enabled).bind(&material.cookies).bind(&material.refresh_token).bind(&material.access_token)
        .bind(material.reauth_config.as_ref().map(serde_json::to_string).transpose()?).bind(i64::from(changed))
        .bind(crate::database::time::now_ms()).bind(id).bind(expected_version).execute(&mut *connection).await?;
    if changed {
        sqlx::query("DELETE FROM credential_profile_health WHERE profile_id = ?")
            .bind(id)
            .execute(&mut *connection)
            .await?;
    }
    load(connection, id).await
}

pub(crate) async fn require_platform(
    connection: &mut SqliteConnection,
    platform_id: &str,
) -> Result<String> {
    sqlx::query_scalar("SELECT platform_name FROM platform_config WHERE id = ?")
        .bind(platform_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(|| Error::not_found("Platform", platform_id))
}

/// The scope that owns a selection must exist on the platform it selects for:
/// a missing or retired owner is not found, one on another platform is invalid.
pub(crate) async fn require_owner(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
) -> Result<String> {
    let platform = require_platform(connection, platform_id).await?;
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
    platform_id: &str,
    label: &str,
    enabled: bool,
    material: &CredentialMaterial,
    route: &ProxyRoute,
) -> Result<CredentialProfile> {
    let platform = require_platform(connection, platform_id).await?;
    material.validate(&platform)?;
    let label = validate_label(label)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::database::time::now_ms();
    sqlx::query("INSERT INTO credential_profiles(id, platform_config_id, label, enabled, cookies, refresh_token, access_token, reauth_config, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&id).bind(platform_id).bind(label).bind(enabled).bind(&material.cookies).bind(&material.refresh_token).bind(&material.access_token)
        .bind(material.reauth_config.as_ref().map(serde_json::to_string).transpose()?).bind(now).bind(now).execute(&mut *connection).await?;
    if !route.is_inherit() {
        super::proxies::set_route(
            connection,
            &super::proxies::RouteOwner::Account(id.clone()),
            route,
        )
        .await?;
    }
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
    {
        return Err(ProfileError::SourceChanged.into());
    }
    require_platform(connection, &current.platform_config_id).await?;
    Ok(())
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
        if profile.platform_config_id != platform_id {
            return Err(ProfileError::InvalidOwner.into());
        }
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

/// The sites `profile_id` names, sorted.
pub(crate) async fn sites_in(
    connection: &mut SqliteConnection,
    profile_id: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT site FROM credential_profile_sites WHERE profile_id = ? ORDER BY site",
    )
    .bind(profile_id)
    .fetch_all(connection)
    .await?)
}

/// Caller owns BEGIN IMMEDIATE. Replaces the sites `profile_id` names and
/// returns whether they changed. A site another account names is refused,
/// naming that account.
pub(crate) async fn set_sites(
    connection: &mut SqliteConnection,
    profile_id: &str,
    sites: &[String],
) -> Result<bool> {
    let current = load(&mut *connection, profile_id).await?;
    let platform = require_platform(&mut *connection, &current.platform_config_id).await?;
    let sites = crate::credentials::normalize_sites(&platform, sites)?;
    if sites_in(&mut *connection, profile_id).await? == sites {
        return Ok(false);
    }
    for site in &sites {
        let holder: Option<(String, String)> = sqlx::query_as("SELECT p.id, p.label FROM credential_profile_sites s JOIN credential_profiles p ON p.id = s.profile_id WHERE s.site = ? AND s.profile_id != ?")
            .bind(site)
            .bind(profile_id)
            .fetch_optional(&mut *connection)
            .await?;
        if let Some((holder, label)) = holder {
            return Err(ProfileError::SiteTaken {
                site: site.clone(),
                profile_id: holder,
                label,
            }
            .into());
        }
    }
    sqlx::query("DELETE FROM credential_profile_sites WHERE profile_id = ?")
        .bind(profile_id)
        .execute(&mut *connection)
        .await?;
    for site in &sites {
        sqlx::query("INSERT INTO credential_profile_sites(site, profile_id) VALUES (?, ?)")
            .bind(site)
            .bind(profile_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(true)
}

/// The accounts on `platform_id` whose sites cover `url`, as (site, profile)
/// pairs, most specific site first. Accounts an import is retiring are left
/// out.
pub(crate) async fn site_accounts(
    connection: &mut SqliteConnection,
    platform_id: &str,
    url: &str,
) -> Result<Vec<(String, String)>> {
    let covering = crate::credentials::covering_sites(url);
    if covering.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as("SELECT s.site, s.profile_id FROM credential_profile_sites s JOIN credential_profiles p ON p.id = s.profile_id WHERE p.platform_config_id = ? AND s.site IN (SELECT value FROM json_each(?)) AND NOT EXISTS(SELECT 1 FROM retirement_credential_profiles r WHERE r.profile_id = p.id) ORDER BY length(s.site) DESC")
        .bind(platform_id)
        .bind(serde_json::to_string(&covering)?)
        .fetch_all(connection)
        .await?)
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
    let streamer: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT platform_config_id, template_config_id FROM streamers WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(streamer_id)
    .fetch_optional(&mut *connection)
    .await?;
    let (platform_id, template_id) = streamer.ok_or(ProfileError::SourceChanged)?;
    let layers = super::credential_selections::load_for_streamer(
        connection,
        streamer_id,
        &platform_id,
        template_id.as_deref(),
    )
    .await?;
    let resolved = crate::credentials::resolve_authentication(&platform_id, &layers)?;
    if resolved.as_ref() != Some(&incoming.policy) {
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

/// The selections that list the profile and the live recordings bound to it.
pub(crate) async fn references(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<ProfileReferences> {
    Ok(references_of(connection, &[id.to_owned()])
        .await?
        .remove(id)
        .unwrap_or_default())
}

#[derive(sqlx::FromRow)]
struct OpenBindingRow {
    id: String,
    streamer_id: Option<String>,
    streamer_name: String,
    credential_binding: String,
}

/// [`references`] of several profiles at once: one query for the selections
/// and one scan of the open sessions, whatever the number of profiles.
/// Profiles without references have no entry.
pub(crate) async fn references_of(
    connection: &mut SqliteConnection,
    ids: &[String],
) -> Result<HashMap<String, ProfileReferences>> {
    let mut references: HashMap<String, ProfileReferences> = HashMap::new();
    if ids.is_empty() {
        return Ok(references);
    }
    for (profile_id, selection) in super::credential_selections::referring(connection, ids).await? {
        references
            .entry(profile_id)
            .or_default()
            .selections
            .push(selection);
    }
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let sessions: Vec<OpenBindingRow> = sqlx::query_as("SELECT ls.id, ls.streamer_id, COALESCE(s.name, ls.streamer_name, ls.streamer_id, '') AS streamer_name, ls.credential_binding FROM live_sessions ls LEFT JOIN streamers s ON s.id = ls.streamer_id WHERE ls.end_time IS NULL AND ls.credential_binding IS NOT NULL ORDER BY ls.start_time, ls.id").fetch_all(&mut *connection).await?;
    for session in sessions {
        let binding: crate::credentials::CredentialBinding =
            serde_json::from_str(&session.credential_binding)?;
        if let crate::credentials::CredentialIdentity::Profile { profile_id } = binding.identity
            && wanted.contains(profile_id.as_str())
        {
            references
                .entry(profile_id)
                .or_default()
                .recordings
                .push(RecordingReference {
                    session_id: session.id,
                    streamer_id: session.streamer_id,
                    streamer_name: session.streamer_name,
                });
        }
    }
    Ok(references)
}

/// Account material lives in profiles; configuration that still carried it
/// would be silently ignored, so writes containing it are rejected.
fn configuration_carries_authentication(raw: Option<&str>, platform: &str) -> bool {
    raw.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|value| crate::credentials::carries_authentication_fields(platform, &value))
}

fn authentication_rejected() -> Error {
    Error::validation(
        "Cookies, tokens and account logins belong in credential profiles, not in configuration",
    )
}

pub(crate) fn reject_platform_authentication(
    model: &crate::database::models::PlatformConfigDbModel,
) -> Result<()> {
    if configuration_carries_authentication(
        model.platform_specific_config.as_deref(),
        &model.platform_name,
    ) {
        return Err(authentication_rejected());
    }
    Ok(())
}

pub(crate) fn reject_template_authentication(
    model: &crate::database::models::TemplateConfigDbModel,
) -> Result<()> {
    let overrides: serde_json::Value = model
        .platform_overrides
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    if overrides.as_object().is_some_and(|entries| {
        entries.iter().any(|(platform, entry)| {
            crate::credentials::carries_authentication_fields(platform, entry)
        })
    }) {
        return Err(authentication_rejected());
    }
    Ok(())
}

pub(crate) async fn reject_streamer_authentication(
    connection: &mut SqliteConnection,
    model: &crate::database::models::StreamerDbModel,
) -> Result<()> {
    let platform: Option<String> =
        sqlx::query_scalar("SELECT platform_name FROM platform_config WHERE id = ?")
            .bind(&model.platform_config_id)
            .fetch_optional(connection)
            .await?;
    if configuration_carries_authentication(
        model.streamer_specific_config.as_deref(),
        platform.as_deref().unwrap_or_default(),
    ) {
        return Err(authentication_rejected());
    }
    Ok(())
}

/// Caller owns BEGIN IMMEDIATE. A platform may be deleted only once no
/// streamer uses it and no template selects accounts on it; its own selection
/// and profiles go with it, unless an active recording still uses a profile.
pub(crate) async fn delete_platform_profiles(
    connection: &mut SqliteConnection,
    platform_id: &str,
) -> Result<()> {
    let streamer_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM streamers WHERE platform_config_id = ? ORDER BY id")
            .bind(platform_id)
            .fetch_all(&mut *connection)
            .await?;
    let template_ids: Vec<String> = super::credential_selections::load_all(connection)
        .await?
        .into_iter()
        .filter_map(|stored| match stored.owner {
            CredentialOwner::Template { template_id } if stored.platform_id == platform_id => {
                Some(template_id)
            }
            _ => None,
        })
        .collect();
    if !streamer_ids.is_empty() || !template_ids.is_empty() {
        return Err(ProfileError::PlatformInUse {
            streamer_ids,
            template_ids,
        }
        .into());
    }
    super::credential_selections::clear_owner(
        connection,
        &CredentialOwner::Platform {
            platform_id: platform_id.to_owned(),
        },
    )
    .await?;
    let profiles: Vec<String> =
        sqlx::query_scalar("SELECT id FROM credential_profiles WHERE platform_config_id = ?")
            .bind(platform_id)
            .fetch_all(&mut *connection)
            .await?;
    let mut referenced = references_of(connection, &profiles).await?;
    let mut blocking = ProfileReferences::default();
    for id in &profiles {
        if let Some(references) = referenced.remove(id) {
            blocking.extend(references);
        }
    }
    if !blocking.is_empty() {
        return Err(ProfileError::Referenced(blocking).into());
    }
    sqlx::query("DELETE FROM credential_profiles WHERE platform_config_id = ?")
        .bind(platform_id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
