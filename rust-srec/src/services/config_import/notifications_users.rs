//! Notification channel and user account import persistence.

use std::collections::HashSet;

use super::snapshot::{ImportSnapshot, includes_imported_users};
use super::{ConfigurationImportError, db_json, proxies, validation_error, write_error};
use crate::config::backup::{ConfigExport, ImportStats};
use crate::database::models::{ChannelType, NotificationChannelDbModel, UserDbModel};
use crate::database::repositories::notification::import_channel as persist_channel;
use crate::database::repositories::user::import_user as persist_user;
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

pub(super) async fn apply_users(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    if !includes_imported_users(config) {
        return Ok(());
    }

    if replace {
        crate::database::repositories::user::release_email_slots(
            tx,
            crate::database::repositories::user::EmailSlots::All,
        )
        .await?;
    } else {
        for user in &config.users {
            if let Some(existing) = snapshot.users.get(&user.username) {
                crate::database::repositories::user::release_email_slots(
                    tx,
                    crate::database::repositories::user::EmailSlots::User(&existing.id),
                )
                .await?;
            }
        }
    }

    for item in &config.users {
        let existing = snapshot.users.get(&item.username);
        let roles = serde_json::to_string(&item.roles).map_err(|error| {
            validation_error(format!(
                "Failed to serialize roles for user '{}': {}",
                item.username, error
            ))
        })?;
        let model = UserDbModel {
            id: existing
                .map(|user| user.id.clone())
                .unwrap_or_else(|| item.id.clone()),
            username: item.username.clone(),
            password_hash: item.password_hash.clone(),
            email: item.email.clone(),
            roles,
            is_active: item.is_active,
            must_change_password: item.must_change_password,
            last_login_at: item.last_login_at,
            created_at: existing
                .map(|user| user.created_at)
                .unwrap_or(item.created_at),
            updated_at: item.updated_at,
        };
        persist_user(tx, &model).await?;
        if existing.is_some() {
            stats.users_updated += 1;
        } else {
            stats.users_created += 1;
        }
    }

    if replace {
        let imported: HashSet<&str> = config
            .users
            .iter()
            .map(|item| item.username.as_str())
            .collect();
        for (username, existing) in &snapshot.users {
            if !imported.contains(username.as_str()) {
                crate::database::repositories::user::delete_user(tx, &existing.id).await?;
                stats.users_deleted += 1;
            }
        }
    }
    Ok(())
}
