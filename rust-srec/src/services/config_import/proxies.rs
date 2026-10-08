//! Proxies are named in backups and resolved to entry IDs inside the import
//! transaction. A bundle's entry updates the installation's entry of the same
//! name, or the one reaching the same exit, or is created.

use std::collections::{HashMap, HashSet};

use crate::config::backup::{BackupRoute, ConfigExport, ImportMode};
use crate::database::repositories::proxies::{self, ProxyUpdate, RouteOwner};
use crate::proxies::{ProxyEndpoint, ProxyRoute, canonical, name_key};

use super::{ConfigurationImportError, validation, validation_error, write_error};

/// Entry IDs by name key: the installation's entries and the bundle's.
#[derive(Debug, Default)]
pub(super) struct ImportedProxies {
    ids: HashMap<String, String>,
    /// Entries whose address or username the import changed: they reach
    /// another exit, so a throttle seen on the old one no longer applies.
    pub(super) moved: Vec<String>,
}

impl ImportedProxies {
    fn route(&self, route: &BackupRoute) -> Result<ProxyRoute, ConfigurationImportError> {
        route
            .to_route(&self.ids)
            .map_err(|name| validation_error(format!("Unknown proxy '{name}'")))
    }

    /// Writes a scope's route. A merge keeps the stored route of a scope the
    /// bundle gives none; a replace makes it inherit, except the global
    /// route, which cannot.
    pub(super) async fn write_route(
        &self,
        tx: &mut sqlx::SqliteConnection,
        owner: &RouteOwner,
        route: Option<&BackupRoute>,
        replace: bool,
    ) -> Result<(), ConfigurationImportError> {
        let route = match route {
            Some(route) => self.route(route)?,
            None if replace && *owner != RouteOwner::Global => ProxyRoute::Inherit,
            None => return Ok(()),
        };
        proxies::set_route(tx, owner, &route)
            .await
            .map_err(write_error)
    }
}

/// The installation's entries, which a converted older backup reuses.
pub(super) async fn existing(
    tx: &mut sqlx::SqliteConnection,
) -> Result<Vec<(String, ProxyEndpoint)>, ConfigurationImportError> {
    Ok(proxies::list(tx)
        .await
        .map_err(write_error)?
        .into_iter()
        .map(|entry| (entry.name.clone(), entry.endpoint()))
        .collect())
}

/// Bundle entries must have distinct names and exits and addresses every
/// client can use.
pub(super) fn validate(config: &ConfigExport) -> Result<(), ConfigurationImportError> {
    if config.version.starts_with("0.") && !config.proxies.is_empty() {
        return validation("Saved proxies require backup schema 1.0.0");
    }
    let mut names = HashSet::new();
    let mut exits = HashSet::new();
    for proxy in &config.proxies {
        if !names.insert(name_key(&proxy.name)) {
            return validation(format!("Duplicate proxy name '{}'", proxy.name));
        }
        let endpoint = canonical(&proxy.endpoint())
            .map_err(|error| validation_error(format!("Proxy '{}': {error}", proxy.name)))?;
        if !exits.insert((endpoint.url, endpoint.username)) {
            return validation(format!(
                "Proxy '{}' reaches the same address and username as another proxy",
                proxy.name
            ));
        }
    }
    Ok(())
}

/// Writes the bundle's entries before any route names them.
pub(super) async fn upsert(
    tx: &mut sqlx::SqliteConnection,
    config: &ConfigExport,
) -> Result<ImportedProxies, ConfigurationImportError> {
    let mut moved = Vec::new();
    for proxy in &config.proxies {
        let endpoint = canonical(&proxy.endpoint())
            .map_err(|error| validation_error(format!("Proxy '{}': {error}", proxy.name)))?;
        let current = match proxies::find_by_name(tx, &proxy.name)
            .await
            .map_err(write_error)?
        {
            Some(entry) => Some(entry),
            None => proxies::find_by_exit(tx, &endpoint)
                .await
                .map_err(write_error)?,
        };
        match current {
            Some(entry) if entry.endpoint() == endpoint => {}
            Some(entry) => {
                // Accounts pinned to an entry whose exit changes start a new
                // revision with unknown health, as with an edit through the API.
                let updated = proxies::update(
                    tx,
                    &entry.id,
                    entry.version,
                    ProxyUpdate {
                        name: None,
                        url: Some(endpoint.url.clone()),
                        username: Some(endpoint.username.clone()),
                        password: endpoint.password.clone(),
                    },
                )
                .await
                .map_err(write_error)?;
                if updated.exit_changed {
                    moved.push(entry.id);
                }
            }
            None => {
                proxies::create(tx, None, &proxy.name, &endpoint)
                    .await
                    .map_err(write_error)?;
            }
        }
    }
    let mut imported = ImportedProxies {
        moved,
        ..ImportedProxies::default()
    };
    for entry in proxies::list(tx).await.map_err(write_error)? {
        imported.ids.insert(name_key(&entry.name), entry.id.clone());
        if let Some(proxy) = config.proxies.iter().find(|proxy| {
            canonical(&proxy.endpoint()).is_ok_and(|endpoint| {
                endpoint.url == entry.url && endpoint.username == entry.username
            })
        }) {
            // A bundle entry that matched by exit is known under its bundle
            // name too.
            imported.ids.insert(name_key(&proxy.name), entry.id);
        }
    }
    Ok(imported)
}

/// A replace removes the installation's entries the bundle omits, once
/// nothing names them; entries still in use stay.
pub(super) async fn delete_omitted(
    tx: &mut sqlx::SqliteConnection,
    config: &ConfigExport,
    mode: ImportMode,
) -> Result<(), ConfigurationImportError> {
    if mode != ImportMode::Replace {
        return Ok(());
    }
    let kept: HashSet<String> = config
        .proxies
        .iter()
        .map(|proxy| name_key(&proxy.name))
        .collect();
    let usage = proxies::usage_counts(tx).await.map_err(write_error)?;
    for entry in proxies::list(tx).await.map_err(write_error)? {
        let named = kept.contains(&name_key(&entry.name))
            || config.proxies.iter().any(|proxy| {
                canonical(&proxy.endpoint()).is_ok_and(|endpoint| {
                    endpoint.url == entry.url && endpoint.username == entry.username
                })
            });
        if !named && usage.get(&entry.id).copied().unwrap_or_default() == 0 {
            proxies::delete(tx, &entry.id, None)
                .await
                .map_err(write_error)?;
        }
    }
    Ok(())
}
