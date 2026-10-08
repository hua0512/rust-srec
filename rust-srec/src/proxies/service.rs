//! Saving proxies and applying their effects on the runtime.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::Result;
use crate::config::ConfigService;
use crate::credentials::{CredentialOwner, PlatformAdmission};
use crate::database::repositories::proxies::{self, ProxyUpdate, RouteOwner};
use crate::database::repositories::{
    CredentialProfileRepository, SqlxConfigRepository, SqlxStreamerRepository,
};

use super::{
    ProxyEndpoint, ProxyEntry, ProxyReferences, ResolvedRoute, RouteKey, RouteSource, SystemProxy,
};

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;

/// Which setting's effective route to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteScope {
    Global,
    Platform {
        platform_id: String,
    },
    /// A template applies per platform; without one it resolves to its own
    /// route, then the global route.
    Template {
        template_id: String,
        platform_id: Option<String>,
    },
    Streamer {
        streamer_id: String,
    },
    /// The route the account's own checks and sign-ins take.
    Account {
        profile_id: String,
    },
}

/// Saved proxies with their runtime effects: resolved configuration is
/// dropped so the next resolution sees an edit, accounts pinned to a moved
/// exit start a new revision, and a moved exit's throttle is forgotten.
pub struct ProxyService {
    pool: SqlitePool,
    write_pool: SqlitePool,
    config_service: Arc<RuntimeConfigService>,
    profiles: Arc<CredentialProfileRepository>,
    admission: Arc<PlatformAdmission>,
    supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
}

impl ProxyService {
    pub fn new(
        pool: SqlitePool,
        write_pool: SqlitePool,
        config_service: Arc<RuntimeConfigService>,
        profiles: Arc<CredentialProfileRepository>,
        admission: Arc<PlatformAdmission>,
    ) -> Self {
        Self {
            pool,
            write_pool,
            config_service,
            profiles,
            admission,
            supervisor: Arc::new(
                crate::utils::task_supervisor::TaskSupervisor::for_committed_work(),
            ),
        }
    }

    pub(crate) fn with_supervisor(
        mut self,
        supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
    ) -> Self {
        self.supervisor = supervisor;
        self
    }

    /// Every entry with how many routes name it.
    pub async fn list(&self) -> Result<Vec<(ProxyEntry, usize)>> {
        let mut connection = self.pool.acquire().await?;
        let entries = proxies::list(&mut connection).await?;
        let usage = proxies::usage_counts(&mut connection).await?;
        Ok(entries
            .into_iter()
            .map(|entry| {
                let count = usage.get(&entry.id).copied().unwrap_or_default();
                (entry, count)
            })
            .collect())
    }

    /// One entry with the routes naming it.
    pub async fn get(&self, id: &str) -> Result<(ProxyEntry, ProxyReferences)> {
        let mut connection = self.pool.acquire().await?;
        let entry = proxies::get(&mut connection, id).await?;
        let references = proxies::references_of(&mut connection, id).await?;
        Ok((entry, references))
    }

    pub async fn create(&self, name: &str, endpoint: &ProxyEndpoint) -> Result<ProxyEntry> {
        let pool = self.write_pool.clone();
        let name = name.to_owned();
        let endpoint = endpoint.clone();
        let entry =
            crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
                let mut tx = crate::database::begin_immediate(&pool).await?;
                let entry = proxies::create(&mut tx, None, &name, &endpoint).await?;
                crate::database::committed_writer::prepare_owned_commit()?;
                tx.commit().await?;
                Ok(entry)
            })
            .await?;
        tracing::info!(proxy_id = %entry.id, name = %entry.name, "Proxy saved");
        Ok(entry)
    }

    pub async fn update(
        &self,
        id: &str,
        expected_version: i64,
        update: ProxyUpdate,
    ) -> Result<ProxyEntry> {
        let pool = self.write_pool.clone();
        let id = id.to_owned();
        let config_service = self.config_service.clone();
        let profiles = self.profiles.clone();
        let admission = self.admission.clone();
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let mut tx = crate::database::begin_immediate(&pool).await?;
            let updated = proxies::update(&mut tx, &id, expected_version, update).await?;
            crate::database::committed_writer::prepare_owned_commit()?;
            tx.commit().await?;
            if updated.exit_changed {
                // The exit is another network path now; a throttle seen on
                // the old one says nothing about it.
                admission.clear_route(&RouteKey::Proxy { id: id.clone() });
                for platform_id in &updated.pinned_platforms {
                    profiles.publish_material(CredentialOwner::Platform {
                        platform_id: platform_id.clone(),
                    });
                }
            }
            config_service.notify_proxies_changed();
            tracing::info!(
                proxy_id = %id,
                exit_changed = updated.exit_changed,
                pinned_platforms = updated.pinned_platforms.len(),
                "Proxy updated"
            );
            Ok(updated.entry)
        })
        .await
    }

    pub async fn delete(&self, id: &str, expected_version: Option<i64>) -> Result<()> {
        let pool = self.write_pool.clone();
        let id = id.to_owned();
        let admission = self.admission.clone();
        let config_service = self.config_service.clone();
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            let mut tx = crate::database::begin_immediate(&pool).await?;
            proxies::delete(&mut tx, &id, expected_version).await?;
            crate::database::committed_writer::prepare_owned_commit()?;
            tx.commit().await?;
            admission.clear_route(&RouteKey::Proxy { id: id.clone() });
            config_service.notify_proxies_changed();
            tracing::info!(proxy_id = %id, "Proxy deleted");
            Ok(())
        })
        .await
    }

    /// The route requests of `scope` take now.
    pub async fn effective(&self, scope: &RouteScope) -> Result<ResolvedRoute> {
        let system = SystemProxy::current();
        let mut connection = self.pool.acquire().await?;
        let layers = match scope {
            RouteScope::Global => proxies::scope_layers(&mut connection, None).await?,
            RouteScope::Platform { platform_id } => {
                proxies::scope_layers(&mut connection, Some(platform_id)).await?
            }
            RouteScope::Template {
                template_id,
                platform_id,
            } => {
                let mut layers = vec![(
                    RouteSource::Template,
                    proxies::route_of(&mut connection, &RouteOwner::Template(template_id.clone()))
                        .await?,
                )];
                layers
                    .extend(proxies::scope_layers(&mut connection, platform_id.as_deref()).await?);
                layers
            }
            RouteScope::Streamer { streamer_id } => {
                drop(connection);
                return Ok(self
                    .config_service
                    .get_context_for_streamer(streamer_id)
                    .await?
                    .config
                    .proxy_route
                    .clone());
            }
            RouteScope::Account { profile_id } => {
                let profile = self.profiles.get(profile_id).await?;
                return proxies::resolve_account(
                    &mut connection,
                    &profile.platform_config_id,
                    &profile.route()?,
                    None,
                    system,
                )
                .await;
            }
        };
        proxies::resolve_layers(&mut connection, &layers, system).await
    }

    /// The saved login of an entry, for checks of an edited entry that keep
    /// its password.
    pub async fn endpoint(&self, id: &str) -> Result<ProxyEndpoint> {
        let mut connection = self.pool.acquire().await?;
        Ok(proxies::get(&mut connection, id).await?.endpoint())
    }
}
