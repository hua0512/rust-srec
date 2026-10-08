//! Profiles are keyed by platform name in backups and resolved to platform IDs
//! inside the import transaction.

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::config::backup::{
    BackupRoute, ConfigExport, CredentialProfileExport, ImportMode,
    MANAGED_CREDENTIAL_SCHEMA_VERSION,
};
use crate::credentials::{CredentialMaterial, CredentialProfile};
use crate::database::repositories::credential_profiles;
use crate::database::repositories::credential_selections::SELECTION_KEY;
use crate::database::repositories::proxies::RouteOwner;

use super::{ConfigurationImportError, validate_unique, validation, validation_error};

#[derive(sqlx::FromRow)]
struct ProfileExportRow {
    #[sqlx(flatten)]
    profile: CredentialProfile,
    platform_name: String,
}

/// Profiles with their routes, naming proxies through `proxy_names` (entry
/// ID to name).
pub(super) async fn export_profiles(
    pool: &SqlitePool,
    proxy_names: &HashMap<String, String>,
) -> crate::Result<Vec<CredentialProfileExport>> {
    let profiles = sqlx::query_as::<_, ProfileExportRow>(
        "SELECT p.*, c.platform_name FROM credential_profiles p JOIN platform_config c ON c.id = p.platform_config_id WHERE NOT EXISTS(SELECT 1 FROM retirement_credential_profiles r WHERE r.profile_id = p.id) ORDER BY p.id"
    ).fetch_all(pool).await?;
    let mut result = Vec::with_capacity(profiles.len());
    for row in profiles {
        let profile = row.profile;
        let material = profile.material()?;
        let proxy_route = BackupRoute::from_route(&profile.route()?, proxy_names)?;
        result.push(CredentialProfileExport {
            id: profile.id,
            platform: row.platform_name,
            label: profile.label,
            enabled: profile.enabled,
            cookies: material.cookies,
            refresh_token: material.refresh_token,
            access_token: material.access_token,
            reauth_config: material.reauth_config,
            proxy_route: Some(proxy_route),
        });
    }
    Ok(result)
}

pub(super) fn validate(config: &ConfigExport) -> Result<(), ConfigurationImportError> {
    if config.version.starts_with("0.") && config.has_managed_credentials() {
        return validation("Managed credentials require backup schema 1.0.0");
    }
    let validate_selection = |value: &serde_json::Value| -> Result<(), ConfigurationImportError> {
        if let Some(selection) = value.get(SELECTION_KEY) {
            if config.version != MANAGED_CREDENTIAL_SCHEMA_VERSION {
                return validation("Managed credentials require backup schema 1.0.0");
            }
            crate::credentials::CredentialSelection::from_value(selection.clone())
                .map_err(|error| validation_error(error.to_string()))?;
        }
        Ok(())
    };
    for template in &config.templates {
        if let Some(overrides) = &template.platform_overrides {
            let overrides = crate::config::backup::unwrap_json_value(overrides.clone());
            if let Some(entries) = overrides.as_object() {
                for value in entries.values() {
                    validate_selection(value)?;
                }
            }
        }
    }
    for streamer in &config.streamers {
        if let Some(value) = &streamer.streamer_specific_config {
            validate_selection(&crate::config::backup::unwrap_json_value(value.clone()))?;
        }
    }
    validate_unique(
        "credential profile ID",
        config.credential_profiles.iter().map(|p| p.id.as_str()),
        false,
    )?;
    for profile in &config.credential_profiles {
        if uuid::Uuid::parse_str(&profile.id).is_err() {
            return validation("Credential profile IDs must be UUIDs");
        }
        if profile.label.trim().is_empty() || profile.label.trim().chars().count() > 128 {
            return validation("Credential profile labels must contain 1..128 characters");
        }
        material(profile)
            .validate(&profile.platform)
            .map_err(|error| validation_error(error.to_string()))?;
    }
    Ok(())
}

fn material(profile: &CredentialProfileExport) -> CredentialMaterial {
    CredentialMaterial {
        cookies: profile.cookies.clone(),
        refresh_token: profile.refresh_token.clone(),
        access_token: profile.access_token.clone(),
        reauth_config: profile.reauth_config.clone(),
    }
}

/// Write the bundle's profiles. Runs before configuration so the selections
/// imported with it can name them, and after proxies so their routes can.
pub(super) async fn upsert(
    tx: &mut sqlx::SqliteConnection,
    config: &ConfigExport,
    platform_ids: &HashMap<String, String>,
    proxies: &super::proxies::ImportedProxies,
    replace: bool,
) -> Result<(), ConfigurationImportError> {
    for profile in &config.credential_profiles {
        let platform_id = platform_ids
            .get(&profile.platform)
            .ok_or_else(|| validation_error("Unknown credential profile platform"))?;
        let current = sqlx::query_as::<_, CredentialProfile>(
            "SELECT * FROM credential_profiles WHERE id = ?",
        )
        .bind(&profile.id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(current) = &current
            && current.platform_config_id != *platform_id
        {
            return validation("Credential profile UUID belongs to a different platform");
        }
        let now = crate::database::time::now_ms();
        // Import replaces material even when its bytes compare equal. In-flight results
        // from before the committed import must not overwrite a restored account.
        sqlx::query("INSERT INTO credential_profiles(id, platform_config_id, label, enabled, cookies, refresh_token, access_token, reauth_config, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET label = excluded.label, enabled = excluded.enabled, cookies = excluded.cookies, refresh_token = excluded.refresh_token, access_token = excluded.access_token, reauth_config = excluded.reauth_config, revision = credential_profiles.revision + 1, version = credential_profiles.version + 1, updated_at = excluded.updated_at")
            .bind(&profile.id).bind(platform_id).bind(profile.label.trim()).bind(profile.enabled)
            .bind(&profile.cookies).bind(&profile.refresh_token).bind(&profile.access_token).bind(profile.reauth_config.as_ref().map(serde_json::Value::to_string)).bind(now).bind(now).execute(&mut *tx).await?;
        proxies
            .write_route(
                tx,
                &RouteOwner::Account(profile.id.clone()),
                profile.proxy_route.as_ref(),
                replace,
            )
            .await?;
        sqlx::query("DELETE FROM credential_profile_health WHERE profile_id = ?")
            .bind(&profile.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM retirement_credential_profiles WHERE profile_id = ?")
            .bind(&profile.id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Replace mode retires the profiles a `1.0.0` bundle omits. Runs after the
/// configuration writes, which removed the selections of retired streamers
/// and templates, so a remaining selection is retained configuration.
pub(super) async fn retire_omitted(
    tx: &mut sqlx::SqliteConnection,
    config: &ConfigExport,
    mode: ImportMode,
) -> Result<(), ConfigurationImportError> {
    if mode == ImportMode::Replace && config.version == MANAGED_CREDENTIAL_SCHEMA_VERSION {
        let imported: HashSet<&str> = config
            .credential_profiles
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        let existing: Vec<String> = sqlx::query_scalar("SELECT id FROM credential_profiles")
            .fetch_all(&mut *tx)
            .await?;
        let omitted: Vec<String> = existing
            .into_iter()
            .filter(|id| !imported.contains(id.as_str()))
            .collect();
        let references = credential_profiles::references_of(tx, &omitted).await?;
        for id in omitted {
            // Active recordings keep the material until they settle; a surviving
            // selection is a conflict, not permission to change it silently.
            if references
                .get(&id)
                .is_some_and(|references| !references.selections.is_empty())
            {
                return validation(
                    "An omitted credential profile is still referenced by retained configuration",
                );
            }
            sqlx::query(
                "INSERT OR IGNORE INTO retirement_credential_profiles(profile_id) VALUES (?)",
            )
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE credential_profiles SET enabled = 0, revision = revision + 1, version = version + 1 WHERE id = ?").bind(&id).execute(&mut *tx).await?;
        }
    }
    crate::database::repositories::config_retirement::reap_retired_profiles(tx).await?;
    Ok(())
}
