//! Streamer and filter import persistence, state transition rules, and retirement marking.

use std::collections::{HashMap, HashSet};

use tracing::warn;

use super::snapshot::ImportSnapshot;
use super::{
    ConfigurationImportError, ImportChanges, db_json, proxies, validation_error, write_error,
};
use crate::config::backup::{ConfigExport, StreamerExport};
use crate::credentials::CredentialOwner;
use crate::database::models::{FilterDbModel, FilterType, StreamerDbModel};
use crate::database::repositories::credential_selections;
use crate::database::repositories::filter::import_filter as persist_filter;
use crate::database::repositories::proxies::RouteOwner;
use crate::database::repositories::streamer::{
    import_streamer_row as persist_streamer_row, mark_streamer_deleted_row,
};
use crate::domain::StreamerState;

/// Streamer rows the import wrote, keyed by what the runtime has to do about
/// them once the transaction commits.
#[derive(Debug, Default)]
pub(super) struct StreamerImportDiff {
    pub(super) upserted: Vec<String>,
    pub(super) marked_deleted: Vec<String>,
    pub(super) rows: Vec<StreamerDbModel>,
    pub(super) state_changed: HashSet<String>,
}

impl StreamerImportDiff {
    fn record_row(&mut self, previous: Option<&StreamerDbModel>, row: StreamerDbModel) {
        let next = crate::streamer::StreamerMetadata::from_db_model(&row);
        let changed = previous.is_none_or(|previous| {
            let previous = crate::streamer::StreamerMetadata::from_db_model(previous);
            previous.state != next.state || previous.is_active() != next.is_active()
        });
        if changed {
            self.state_changed.insert(row.id.clone());
        }
        self.rows.push(row);
    }
}

/// What streamers of a bundle name, resolved to local IDs.
pub(super) struct ImportedScopes<'a> {
    pub(super) templates: &'a HashMap<String, String>,
    pub(super) platforms: &'a HashMap<String, String>,
    pub(super) proxies: &'a proxies::ImportedProxies,
}

pub(super) async fn apply_streamers(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    scopes: &ImportedScopes<'_>,
    changes: &mut ImportChanges,
) -> Result<(), ConfigurationImportError> {
    let ImportedScopes {
        templates: template_ids,
        platforms: platform_ids,
        proxies: imported_proxies,
    } = scopes;
    let mut retained_streamer_ids: HashSet<String> = HashSet::new();
    for item in &config.streamers {
        let existing = snapshot.streamer_for_update(&item.url);
        let platform_id = platform_ids.get(&item.platform).ok_or_else(|| {
            validation_error(format!(
                "Unknown platform '{}' for streamer '{}'",
                item.platform, item.name
            ))
        })?;
        let template_id = item
            .template
            .as_ref()
            .map(|name| {
                template_ids.get(name).cloned().ok_or_else(|| {
                    validation_error(format!(
                        "Unknown template '{}' for streamer '{}'",
                        name, item.name
                    ))
                })
            })
            .transpose()?;
        let model = streamer_model(existing, item, platform_id, template_id);
        if replace && let Some(existing) = existing {
            credential_selections::clear_owner(
                tx,
                &CredentialOwner::Streamer {
                    streamer_id: existing.id.clone(),
                },
            )
            .await
            .map_err(write_error)?;
        }
        if let Some(row) = persist_streamer_row(tx, &model)
            .await
            .map_err(write_error)?
        {
            changes.streamers.record_row(existing, row);
        }
        imported_proxies
            .write_route(
                tx,
                &RouteOwner::Streamer(model.id.clone()),
                item.proxy_route.as_ref(),
                replace,
            )
            .await?;
        crate::database::repositories::filter::delete_for_streamer(tx, &model.id).await?;
        for item_filter in &item.filters {
            let filter_type = FilterType::parse(&item_filter.filter_type).ok_or_else(|| {
                validation_error(format!("Invalid filter type '{}'", item_filter.filter_type))
            })?;
            let filter = FilterDbModel::new(
                &model.id,
                filter_type,
                db_json(crate::config::backup::import_filter_config(
                    &config.version,
                    &item_filter.filter_type,
                    item_filter.config.clone(),
                )),
            );
            persist_filter(tx, &filter).await?;
        }
        retained_streamer_ids.insert(model.id.clone());
        changes.streamers.upserted.push(model.id.clone());
        if existing.is_some() {
            changes.stats.streamers_updated += 1;
        } else {
            changes.stats.streamers_created += 1;
        }
    }
    if !replace {
        for rows in snapshot.streamers.values() {
            if rows.len() > 1 {
                let colliding: Vec<&str> = rows.iter().map(|row| row.url.as_str()).collect();
                warn!(
                    urls = %colliding.join(", "),
                    "Streamer rows share a URL differing only by case; merge import updates at most one of them and keeps the rest"
                );
            }
        }
    }
    if replace {
        let now = crate::database::time::now_ms();
        for rows in snapshot.streamers.values() {
            for existing in rows {
                if retained_streamer_ids.contains(&existing.id) {
                    continue;
                }
                if let Some(row) = mark_streamer_deleted_row(&mut *tx, &existing.id, now).await? {
                    changes.streamers.record_row(Some(existing), row);
                }
                changes.streamers.marked_deleted.push(existing.id.clone());
                changes.stats.streamers_deleted += 1;
            }
        }
    }
    Ok(())
}

fn streamer_model(
    existing: Option<&StreamerDbModel>,
    source: &StreamerExport,
    platform_id: &str,
    template_id: Option<String>,
) -> StreamerDbModel {
    let StreamerExport {
        name,
        url,
        platform: _,
        template: _,
        priority,
        state,
        avatar_url,
        streamer_specific_config,
        proxy_route: _,
        filters: _,
    } = source;
    let mut model = existing
        .cloned()
        .unwrap_or_else(|| StreamerDbModel::new(name, url, platform_id));
    model.name = name.clone();
    model.url = url.clone();
    model.platform_config_id = platform_id.to_string();
    model.template_config_id = template_id;
    model.state = imported_state(existing, state);
    if clears_error_tracking(existing, &model.state) {
        model.consecutive_error_count = Some(0);
        model.disabled_until = None;
        model.last_error = None;
    }
    model.priority = priority.to_ascii_uppercase();
    model.avatar = avatar_url.clone().filter(|value| !value.trim().is_empty());
    model.streamer_specific_config = streamer_specific_config.clone().map(db_json);
    model.updated_at = crate::database::time::now_ms();
    model
}

pub(super) fn imported_state(existing: Option<&StreamerDbModel>, source_state: &str) -> String {
    let not_live = StreamerState::NotLive.as_str().to_string();

    match StreamerState::parse(source_state) {
        Some(StreamerState::Disabled) => StreamerState::Disabled.as_str().to_string(),
        _ => match existing.and_then(|row| StreamerState::parse(&row.state)) {
            Some(StreamerState::Disabled) => not_live,
            Some(current) => current.as_str().to_string(),
            None => not_live,
        },
    }
}

pub(super) fn clears_error_tracking(existing: Option<&StreamerDbModel>, new_state: &str) -> bool {
    match StreamerState::parse(new_state) {
        Some(StreamerState::Disabled) => true,
        Some(StreamerState::NotLive) => existing
            .and_then(|row| StreamerState::parse(&row.state))
            .is_some_and(|state| state == StreamerState::Disabled),
        _ => false,
    }
}
