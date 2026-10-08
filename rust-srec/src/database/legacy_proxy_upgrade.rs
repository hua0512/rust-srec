//! Conversion of the `proxy_config` JSON stored on each scope into saved
//! proxies and routes.
//!
//! The schema migration that adds routes installs
//! `legacy_proxy_upgrade_pending`; [`run`] converts the database and drops the
//! marker in the same transaction, so an interrupted start retries the whole
//! conversion. Imported backups that still carry `proxy_config` go through
//! [`upgrade_bundle`].
//!
//! The `proxy_config` columns are gone; the migration that dropped them
//! moved their values into `legacy_proxy_settings`, which this conversion
//! reads and drops. Each distinct exit (address and username) becomes one
//! entry named after its address. The streamer `proxy_config` keys are
//! removed: nothing reads them.
//! The template `platform_overrides.<platform>.proxy_config` keys were never
//! read and are removed.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::{SqliteConnection, SqlitePool};
use tracing::{info, warn};

use crate::Result;
use crate::config::backup::{BackupRoute, ConfigExport, ProxyExport};
use crate::database::repositories::proxies::{self, LEGACY_KEY, RouteOwner};
use crate::proxies::legacy::{self, LegacyMapping, LegacyRoute, Namer};
use crate::proxies::{ProxyEndpoint, name_key};

const MARKER: &str = "legacy_proxy_upgrade_pending";
const STASH: &str = "legacy_proxy_settings";

/// Converts the database once. `environment_proxy` reports whether proxy
/// environment variables are set now: a global setting that asked for no
/// proxy left download engines on them, so it becomes the system route.
pub(crate) async fn run(pool: &SqlitePool, environment_proxy: bool) -> Result<()> {
    let converted =
        crate::database::run_marked_conversion(pool, MARKER, STASH, async |connection| {
            convert_database(connection, environment_proxy).await
        })
        .await?;
    if let Some(converted) = converted.filter(|converted| converted.routes > 0) {
        info!(
            proxies = converted.proxies,
            routes = converted.routes,
            "Converted proxy settings into saved proxies"
        );
    }
    Ok(())
}

struct Converted {
    proxies: usize,
    routes: usize,
}

/// A scope's converted setting, before entries have IDs.
struct Pending {
    owner: RouteOwner,
    route: LegacyRoute,
}

fn report(kind: &str, id: &str, name: &str, mapping: &LegacyMapping) {
    if let Some(reason) = mapping.warning {
        warn!(
            scope = kind,
            scope_id = id,
            scope_name = name,
            reason,
            "Proxy setting could not be kept as written; the scope now connects as described"
        );
    }
}

fn object(raw: Option<&str>) -> Option<Value> {
    raw.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .filter(Value::is_object)
}

/// Removes `proxy_config` from a JSON object document, returning its value
/// when the key was present.
fn take_key(document: &mut Value) -> Option<Value> {
    document
        .as_object_mut()
        .and_then(|fields| fields.remove(LEGACY_KEY))
}

/// Removes the never-read `proxy_config` of each template platform override.
fn strip_override_keys(overrides: &mut Value) -> bool {
    let mut changed = false;
    if let Some(entries) = overrides.as_object_mut() {
        for entry in entries.values_mut() {
            changed |= take_key(entry).is_some();
        }
    }
    changed
}

async fn convert_database(
    connection: &mut SqliteConnection,
    environment_proxy: bool,
) -> Result<Converted> {
    let existing = proxies::list(connection).await?;
    let existing_endpoints: Vec<(String, ProxyEndpoint)> = existing
        .iter()
        .map(|entry| (entry.name.clone(), entry.endpoint()))
        .collect();
    let mut namer = Namer::new(
        existing_endpoints
            .iter()
            .map(|(name, endpoint)| (name.as_str(), endpoint)),
    );
    let mut ids: HashMap<String, String> = existing
        .iter()
        .map(|entry| (name_key(&entry.name), entry.id.clone()))
        .collect();
    let mut pending = Vec::new();

    let (global_id, global_raw): (String, Option<String>) =
        sqlx::query_as(
            "SELECT g.id, s.proxy_config FROM global_config g LEFT JOIN legacy_proxy_settings s ON s.scope = 'global' AND s.id = g.id ORDER BY g.rowid LIMIT 1",
        )
        .fetch_one(&mut *connection)
        .await?;
    let mapping = legacy::map_text(global_raw.as_deref());
    report("global", &global_id, "global", &mapping);
    pending.push(Pending {
        owner: RouteOwner::Global,
        route: legacy::global_route(mapping, environment_proxy),
    });

    let platforms: Vec<(String, String, Option<String>)> =
        sqlx::query_as(
            "SELECT p.id, p.platform_name, s.proxy_config FROM platform_config p LEFT JOIN legacy_proxy_settings s ON s.scope = 'platform' AND s.id = p.id ORDER BY p.id",
        )
        .fetch_all(&mut *connection)
        .await?;
    for (id, name, raw) in platforms {
        let mapping = legacy::map_text(raw.as_deref());
        report("platform", &id, &name, &mapping);
        pending.push(Pending {
            owner: RouteOwner::Platform(id),
            route: mapping.route,
        });
    }

    // Retiring templates convert too: their recordings still resolve them.
    // Any update of a template cancels its deferred deletion, so the pending
    // deletions are restored once the conversion has written them.
    let retiring: Vec<String> = sqlx::query_scalar(
        "SELECT config_id FROM retirement_config_deletions WHERE kind = 'template'",
    )
    .fetch_all(&mut *connection)
    .await?;
    let templates: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT t.id, t.name, s.proxy_config, t.platform_overrides FROM template_config t LEFT JOIN legacy_proxy_settings s ON s.scope = 'template' AND s.id = t.id ORDER BY t.id",
    )
    .fetch_all(&mut *connection)
    .await?;
    for (id, name, raw, overrides) in templates {
        let mapping = legacy::map_text(raw.as_deref());
        report("template", &id, &name, &mapping);
        if let Some(mut overrides) = object(overrides.as_deref())
            && strip_override_keys(&mut overrides)
        {
            sqlx::query("UPDATE template_config SET platform_overrides = ? WHERE id = ?")
                .bind(overrides.to_string())
                .bind(&id)
                .execute(&mut *connection)
                .await?;
        }
        pending.push(Pending {
            owner: RouteOwner::Template(id),
            route: mapping.route,
        });
    }

    let streamers: Vec<(String, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT id, name, streamer_specific_config, deleted_at IS NOT NULL FROM streamers ORDER BY id",
    )
    .fetch_all(&mut *connection)
    .await?;
    for (id, name, raw, deleted) in streamers {
        let Some(mut document) = object(raw.as_deref()) else {
            continue;
        };
        let Some(value) = take_key(&mut document) else {
            continue;
        };
        sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = ?")
            .bind(document.to_string())
            .bind(&id)
            .execute(&mut *connection)
            .await?;
        // A streamer being removed keeps no route.
        if deleted {
            continue;
        }
        let mapping = legacy::map_value(Some(&value));
        report("streamer", &id, &name, &mapping);
        pending.push(Pending {
            owner: RouteOwner::Streamer(id),
            route: mapping.route,
        });
    }

    for item in &pending {
        if let LegacyRoute::Proxy(endpoint) = &item.route {
            namer.entry_for(endpoint);
        }
    }
    let mut created = 0;
    for entry in namer.created() {
        let saved = proxies::create(connection, None, &entry.name, &entry.endpoint).await?;
        ids.insert(name_key(&entry.name), saved.id);
        created += 1;
    }
    if namer.password_conflicts() > 0 {
        warn!(
            conflicts = namer.password_conflicts(),
            "Some scopes used the same proxy address and username with different passwords; the first password is kept"
        );
    }

    let mut routes = 0;
    for item in pending {
        let route = item
            .route
            .into_backup(|endpoint| namer.entry_for(endpoint).name.clone())
            .to_route(&ids)
            .map_err(|_| crate::Error::Other("converted proxy has no ID".into()))?;
        if route.is_inherit() {
            continue;
        }
        proxies::set_route(connection, &item.owner, &route).await?;
        routes += 1;
    }

    for template_id in retiring {
        sqlx::query(
            "INSERT OR IGNORE INTO retirement_config_deletions(kind, config_id) SELECT 'template', id FROM template_config WHERE id = ?",
        )
        .bind(template_id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(Converted {
        proxies: created,
        routes,
    })
}

/// Converts the `proxy_config` settings an imported backup carries into
/// saved proxies and routes. `existing` lists the installation's entries:
/// an exit one of them already reaches reuses it, and new entries avoid
/// their names. A scope that already carries a route keeps it. Backups
/// written before saved proxies (`every_scope`) always restored each scope's
/// setting, so a scope without one inherits; otherwise only scopes carrying
/// a setting convert. Unlike the database conversion, the environment is not
/// consulted: a restored global setting means what the backup says.
pub(crate) fn upgrade_bundle(
    export: &mut ConfigExport,
    existing: &[(String, ProxyEndpoint)],
    every_scope: bool,
) -> Result<()> {
    let bundled: Vec<(String, ProxyEndpoint)> = export
        .proxies
        .iter()
        .map(|proxy| (proxy.name.clone(), proxy.endpoint()))
        .collect();
    let mut namer = Namer::new(
        existing
            .iter()
            .chain(&bundled)
            .map(|(name, endpoint)| (name.as_str(), endpoint)),
    );
    let mut convert = |route: LegacyRoute| -> BackupRoute {
        route.into_backup(|endpoint| namer.entry_for(endpoint).name.clone())
    };
    let global = &mut export.global_config;
    let raw = std::mem::take(&mut global.proxy_config);
    if global.proxy_route.is_none() && (every_scope || !raw.is_null()) {
        let mapping = legacy::map_value(Some(&raw));
        global.proxy_route = Some(convert(legacy::global_route(mapping, false)));
    }
    for platform in &mut export.platforms {
        let raw = platform.proxy_config.take();
        if platform.proxy_route.is_none() && (every_scope || raw.is_some()) {
            platform.proxy_route = Some(convert(legacy::map_value(raw.as_ref()).route));
        }
    }
    for template in &mut export.templates {
        let raw = template.proxy_config.take();
        if template.proxy_route.is_none() && (every_scope || raw.is_some()) {
            template.proxy_route = Some(convert(legacy::map_value(raw.as_ref()).route));
        }
        if let Some(overrides) = &mut template.platform_overrides {
            let mut value = crate::config::backup::unwrap_json_value(overrides.clone());
            if strip_override_keys(&mut value) {
                *overrides = value;
            }
        }
    }
    for streamer in &mut export.streamers {
        let raw = streamer
            .streamer_specific_config
            .as_mut()
            .and_then(|document| {
                let mut value = crate::config::backup::unwrap_json_value(document.clone());
                let raw = take_key(&mut value)?;
                *document = value;
                Some(raw)
            });
        if streamer.proxy_route.is_none() && (every_scope || raw.is_some()) {
            streamer.proxy_route = Some(convert(legacy::map_value(raw.as_ref()).route));
        }
    }
    let created: Vec<ProxyExport> = namer
        .created()
        .map(|entry| ProxyExport {
            name: entry.name.clone(),
            url: entry.endpoint.url.clone(),
            username: entry.endpoint.username.clone(),
            password: entry.endpoint.password.clone(),
        })
        .collect();
    export.proxies.extend(created);
    Ok(())
}

/// Whether an imported backup still carries proxy settings to convert.
pub(crate) fn bundle_has_legacy_settings(export: &ConfigExport) -> bool {
    !export.global_config.proxy_config.is_null()
        || export
            .platforms
            .iter()
            .any(|platform| platform.proxy_config.is_some())
        || export.templates.iter().any(|template| {
            template.proxy_config.is_some()
                || template
                    .platform_overrides
                    .as_ref()
                    .is_some_and(|overrides| {
                        crate::config::backup::unwrap_json_value(overrides.clone())
                            .as_object()
                            .is_some_and(|entries| {
                                entries
                                    .values()
                                    .any(|entry| entry.get(LEGACY_KEY).is_some())
                            })
                    })
        })
        || export.streamers.iter().any(|streamer| {
            streamer
                .streamer_specific_config
                .as_ref()
                .map(|document| crate::config::backup::unwrap_json_value(document.clone()))
                .is_some_and(|document| document.get(LEGACY_KEY).is_some())
        })
}
