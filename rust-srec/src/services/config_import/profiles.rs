//! The backup's natural owner keys are resolved inside the import transaction.

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::config::backup::{
    ConfigExport, CredentialOwnerExport, CredentialProfileExport, ImportMode,
};
use crate::credentials::{CredentialMaterial, CredentialOwner, CredentialProfile};
use crate::database::repositories::credential_profiles;

use super::{ConfigurationImportError, validate_unique, validation, validation_error};

pub(super) fn preserve_streamer_policy(
    old: &crate::database::models::StreamerDbModel,
    new: &mut crate::database::models::StreamerDbModel,
) -> Result<(), ConfigurationImportError> {
    credential_profiles::preserve_streamer_policy(old, new)
        .map_err(|error| validation_error(error.to_string()))
}

pub(super) fn preserve_template_policies(
    old: &crate::database::models::TemplateConfigDbModel,
    new: &mut crate::database::models::TemplateConfigDbModel,
) -> Result<(), ConfigurationImportError> {
    credential_profiles::preserve_template_policies(old, new)
        .map_err(|error| validation_error(error.to_string()))
}

#[derive(sqlx::FromRow)]
struct ProfileExportRow {
    #[sqlx(flatten)]
    profile: CredentialProfile,
    platform_name: String,
    template_name: Option<String>,
    streamer_url: Option<String>,
}

pub(super) async fn export_profiles(
    pool: &SqlitePool,
) -> crate::Result<Vec<CredentialProfileExport>> {
    let profiles = sqlx::query_as::<_, ProfileExportRow>(
        "SELECT p.*, c.platform_name, t.name AS template_name, s.url AS streamer_url FROM credential_profiles p JOIN platform_config c ON c.id = p.platform_config_id LEFT JOIN template_config t ON t.id = p.template_id LEFT JOIN streamers s ON s.id = p.streamer_id WHERE NOT EXISTS(SELECT 1 FROM retirement_credential_profiles r WHERE r.profile_id = p.id) AND (p.streamer_id IS NULL OR s.deleted_at IS NULL) AND (p.template_id IS NULL OR NOT EXISTS(SELECT 1 FROM retirement_config_deletions r WHERE r.kind = 'template' AND r.config_id = p.template_id)) ORDER BY p.id"
    ).fetch_all(pool).await?;
    let mut result = Vec::with_capacity(profiles.len());
    for row in profiles {
        let profile = row.profile;
        let platform = row.platform_name;
        let owner = match profile.owner()? {
            CredentialOwner::Platform { .. } => CredentialOwnerExport::Platform {
                platform_name: platform.clone(),
            },
            CredentialOwner::Template { .. } => CredentialOwnerExport::Template {
                template_name: row
                    .template_name
                    .ok_or(crate::credentials::ProfileError::InvalidOwner)?,
            },
            CredentialOwner::Streamer { .. } => CredentialOwnerExport::Streamer {
                streamer_url: row
                    .streamer_url
                    .ok_or(crate::credentials::ProfileError::InvalidOwner)?,
            },
        };
        let material = profile.material()?;
        result.push(CredentialProfileExport {
            id: profile.id,
            platform,
            owner,
            label: profile.label,
            enabled: profile.enabled,
            cookies: material.cookies,
            refresh_token: material.refresh_token,
            access_token: material.access_token,
            reauth_config: material.reauth_config,
        });
    }
    Ok(result)
}

pub(super) fn validate(config: &ConfigExport) -> Result<(), ConfigurationImportError> {
    if config.version.starts_with("0.") && config.has_managed_credentials() {
        return validation("Managed credentials require backup schema 1.0.0");
    }
    let validate_selection = |value: &serde_json::Value| -> Result<(), ConfigurationImportError> {
        if let Some(selection) = value.get("credential_selection") {
            if config.version != "1.0.0" {
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

pub(super) async fn apply(
    tx: &mut sqlx::SqliteConnection,
    config: &ConfigExport,
    mode: ImportMode,
    template_ids: &HashMap<String, String>,
    platform_ids: &HashMap<String, String>,
) -> Result<(), ConfigurationImportError> {
    for profile in &config.credential_profiles {
        let platform_id = platform_ids
            .get(&profile.platform)
            .ok_or_else(|| validation_error("Unknown credential profile platform"))?;
        let owner = match &profile.owner {
            CredentialOwnerExport::Platform { platform_name } => {
                if platform_name != &profile.platform {
                    return validation("Credential profile owner/platform mismatch");
                }
                CredentialOwner::Platform {
                    platform_id: platform_id.clone(),
                }
            }
            CredentialOwnerExport::Template { template_name } => CredentialOwner::Template {
                template_id: template_ids
                    .get(template_name)
                    .cloned()
                    .ok_or_else(|| validation_error("Unknown credential profile template owner"))?,
            },
            CredentialOwnerExport::Streamer { streamer_url } => {
                let id = sqlx::query_scalar("SELECT id FROM streamers WHERE url = ? COLLATE NOCASE AND deleted_at IS NULL ORDER BY id LIMIT 1")
                    .bind(streamer_url).fetch_optional(&mut *tx).await?.ok_or_else(|| validation_error("Unknown credential profile streamer owner"))?;
                CredentialOwner::Streamer { streamer_id: id }
            }
        };
        credential_profiles::require_owner(tx, &owner, platform_id)
            .await
            .map_err(|error| validation_error(error.to_string()))?;
        let current = sqlx::query_as::<_, CredentialProfile>(
            "SELECT * FROM credential_profiles WHERE id = ?",
        )
        .bind(&profile.id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(current) = &current
            && (current.platform_config_id != *platform_id
                || current
                    .owner()
                    .map_err(|error| validation_error(error.to_string()))?
                    != owner)
        {
            return validation("Credential profile UUID belongs to a different owner or platform");
        }
        let template_id = match &owner {
            CredentialOwner::Template { template_id } => Some(template_id),
            _ => None,
        };
        let streamer_id = match &owner {
            CredentialOwner::Streamer { streamer_id } => Some(streamer_id),
            _ => None,
        };
        let now = crate::database::time::now_ms();
        // Import replaces material even when its bytes compare equal. In-flight results
        // from before the committed import must not overwrite a restored account.
        sqlx::query("INSERT INTO credential_profiles(id, platform_config_id, owner_kind, template_id, streamer_id, label, enabled, cookies, refresh_token, access_token, reauth_config, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET label = excluded.label, enabled = excluded.enabled, cookies = excluded.cookies, refresh_token = excluded.refresh_token, access_token = excluded.access_token, reauth_config = excluded.reauth_config, revision = credential_profiles.revision + 1, version = credential_profiles.version + 1, updated_at = excluded.updated_at")
            .bind(&profile.id).bind(platform_id).bind(owner.kind()).bind(template_id).bind(streamer_id).bind(profile.label.trim()).bind(profile.enabled)
            .bind(&profile.cookies).bind(&profile.refresh_token).bind(&profile.access_token).bind(profile.reauth_config.as_ref().map(serde_json::Value::to_string)).bind(now).bind(now).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM credential_profile_health WHERE profile_id = ?")
            .bind(&profile.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM retirement_credential_profiles WHERE profile_id = ?")
            .bind(&profile.id)
            .execute(&mut *tx)
            .await?;
    }
    if mode == ImportMode::Replace && config.version == "1.0.0" {
        let imported: HashSet<&str> = config
            .credential_profiles
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        let existing: Vec<String> = sqlx::query_scalar("SELECT id FROM credential_profiles")
            .fetch_all(&mut *tx)
            .await?;
        for id in existing {
            if imported.contains(id.as_str()) {
                continue;
            }
            let references = credential_profiles::references(tx, &id, None).await?;
            // References from retiring owners disappear with their owner. Any surviving
            // policy is a graph conflict, not permission to silently change selection.
            for reference in references.iter().filter(|r| !r.starts_with("session:")) {
                if let Some(streamer_id) = reference.strip_prefix("streamer:") {
                    let retiring: bool = sqlx::query_scalar(
                        "SELECT deleted_at IS NOT NULL FROM streamers WHERE id = ?",
                    )
                    .bind(streamer_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if retiring {
                        continue;
                    }
                }
                if let Some(template_id) = reference.strip_prefix("template:") {
                    let template_id = template_id
                        .rsplit_once(':')
                        .map_or(template_id, |(id, _)| id);
                    let retiring: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM retirement_config_deletions WHERE kind = 'template' AND config_id = ?)").bind(template_id).fetch_one(&mut *tx).await?;
                    if retiring {
                        continue;
                    }
                }
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
    credential_profiles::validate_graph(tx)
        .await
        .map_err(|error| validation_error(error.to_string()))?;
    crate::database::repositories::config_retirement::reap_retired_profiles(tx).await?;
    Ok(())
}
