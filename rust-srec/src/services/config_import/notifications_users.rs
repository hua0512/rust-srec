//! Notification channel import persistence.

use std::collections::HashSet;

use super::snapshot::ImportSnapshot;
use super::{ConfigurationImportError, db_json, proxies, validation_error, write_error};
use crate::config::backup::{ConfigExport, ImportStats};
use crate::database::models::{ChannelType, NotificationChannelDbModel};
use crate::database::repositories::notification::import_channel as persist_channel;
use crate::notification::events::is_retired_subscription_event_name;

pub(super) async fn apply_notification_channels(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    imported_proxies: &proxies::ImportedProxies,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.notification_channels {
        let existing = snapshot.channels.get(&item.name);
        let channel_type = ChannelType::parse(&item.channel_type).ok_or_else(|| {
            validation_error(format!(
                "Invalid notification channel type '{}'",
                item.channel_type
            ))
        })?;
        let proxy_route = match &item.proxy_route {
            Some(route) => imported_proxies.route(route)?,
            None if replace || !channel_type.supports_proxy() => {
                crate::proxies::ProxyRoute::Inherit
            }
            None => existing
                .map(|existing| existing.proxy_route.clone())
                .unwrap_or_default(),
        };
        let model = if let Some(existing) = existing {
            NotificationChannelDbModel {
                id: existing.id.clone(),
                name: item.name.clone(),
                channel_type: channel_type.as_str().to_string(),
                settings: db_json(item.settings.clone()),
                proxy_route,
            }
        } else {
            NotificationChannelDbModel {
                proxy_route,
                ..NotificationChannelDbModel::new(
                    &item.name,
                    channel_type,
                    db_json(item.settings.clone()),
                )
            }
        };
        persist_channel(tx, &model).await.map_err(write_error)?;
        crate::database::repositories::notification::delete_subscriptions(tx, &model.id).await?;
        for event in item
            .subscriptions
            .iter()
            .filter(|event| !is_retired_subscription_event_name(event))
        {
            crate::database::repositories::notification::insert_subscription(
                tx,
                &model.id,
                event,
                crate::database::repositories::notification::SubscriptionInsert::Strict,
            )
            .await?;
        }
        if existing.is_some() {
            stats.channels_updated += 1;
        } else {
            stats.channels_created += 1;
        }
    }

    if replace {
        let imported: HashSet<&str> = config
            .notification_channels
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        for (name, existing) in &snapshot.channels {
            if imported.contains(name.as_str()) {
                continue;
            }
            crate::database::repositories::notification::delete_channel_dead_letters(
                tx,
                &existing.id,
            )
            .await?;
            crate::database::repositories::notification::delete_subscriptions(tx, &existing.id)
                .await?;
            crate::database::repositories::notification::delete_channel_row(tx, &existing.id)
                .await?;
            stats.channels_deleted += 1;
        }
    }
    Ok(())
}
