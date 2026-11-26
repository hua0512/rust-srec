//! Transactional user backup export and configuration-import helpers.

use std::collections::{HashMap, HashSet};

use argon2::password_hash::phc::PasswordHash;

use crate::config::backup::{ImportMode, UserExport};

use super::models::UserDbModel;
use super::refresh_token::invalidate_all_for_import;
use super::user::{EmailSlots, UserRepository, delete_user, import_user, release_email_slots};

/// Error returned while validating or applying auth state inside a configuration
/// import transaction.
#[derive(Debug, thiserror::Error)]
pub enum AuthImportError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// User mutation counts recorded by [`apply_auth_import_tx`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AuthImportStats {
    pub users_created: u32,
    pub users_updated: u32,
    pub users_deleted: u32,
}

async fn load_existing_users(
    tx: &mut sqlx::SqliteConnection,
) -> std::result::Result<HashMap<String, UserDbModel>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UserDbModel>("SELECT * FROM users")
        .fetch_all(tx)
        .await?;
    Ok(rows
        .into_iter()
        .map(|model| (model.username.clone(), model))
        .collect())
}

fn validate_against_existing(
    existing: &HashMap<String, UserDbModel>,
    users: &[UserExport],
    include_users: bool,
    mode: ImportMode,
) -> Result<(), AuthImportError> {
    let existing_ids: HashMap<&str, &str> = existing
        .values()
        .map(|user| (user.id.as_str(), user.username.as_str()))
        .collect();
    for user in users {
        if existing.contains_key(&user.username) {
            continue;
        }
        if let Some(existing_username) = existing_ids.get(user.id.as_str()) {
            return Err(AuthImportError::Validation(format!(
                "User id '{}' is already in use by username '{}'",
                user.id, existing_username
            )));
        }
    }

    if !include_users {
        return Ok(());
    }

    let replaced: HashSet<&str> = users.iter().map(|user| user.username.as_str()).collect();
    let mut emails = HashSet::new();
    if mode == ImportMode::Merge {
        for user in existing
            .values()
            .filter(|user| !replaced.contains(user.username.as_str()))
        {
            if let Some(email) = user.email.as_deref() {
                emails.insert(email);
            }
        }
    }
    for user in users {
        if let Some(email) = user.email.as_deref()
            && !emails.insert(email)
        {
            return Err(AuthImportError::Validation(format!(
                "Imported user '{}' has an email already assigned to another user",
                user.username,
            )));
        }
    }
    Ok(())
}

/// Validate password hashes of imported user records before starting a
/// database transaction.
pub fn validate_user_exports(users: &[UserExport]) -> Result<(), AuthImportError> {
    for user in users {
        let parsed = PasswordHash::new(&user.password_hash).map_err(|error| {
            AuthImportError::Validation(format!(
                "Invalid password hash for user '{}': {}",
                user.username, error
            ))
        })?;
        if parsed.algorithm.as_str() != "argon2id" {
            return Err(AuthImportError::Validation(format!(
                "Password hash for user '{}' must use Argon2id",
                user.username
            )));
        }
    }
    Ok(())
}

/// Validate imported user IDs and emails against persisted `users` rows inside
/// `tx` before any configuration writes execute.
pub async fn validate_auth_import_tx(
    tx: &mut sqlx::SqliteConnection,
    users: &[UserExport],
    include_users: bool,
    mode: ImportMode,
) -> Result<(), AuthImportError> {
    if !include_users && users.is_empty() {
        return Ok(());
    }
    let existing = load_existing_users(tx).await?;
    validate_against_existing(&existing, users, include_users, mode)
}

/// Apply imported user accounts (when `include_users` is true) and invalidate
/// all `auth_sessions` and `refresh_tokens` inside the caller's single SQLite
/// transaction so user writes and session invalidation commit or roll back
/// atomically with the rest of configuration import.
pub async fn apply_auth_import_tx(
    tx: &mut sqlx::SqliteConnection,
    users: &[UserExport],
    include_users: bool,
    mode: ImportMode,
) -> Result<AuthImportStats, AuthImportError> {
    let mut stats = AuthImportStats::default();

    if include_users {
        let existing = load_existing_users(tx).await?;
        validate_against_existing(&existing, users, true, mode)?;

        let replace = mode == ImportMode::Replace;
        if replace {
            release_email_slots(tx, EmailSlots::All).await?;
        } else {
            for user in users {
                if let Some(existing_user) = existing.get(&user.username) {
                    release_email_slots(tx, EmailSlots::User(&existing_user.id)).await?;
                }
            }
        }

        for item in users {
            let existing_user = existing.get(&item.username);
            let roles = serde_json::to_string(&item.roles).map_err(|error| {
                AuthImportError::Validation(format!(
                    "Failed to serialize roles for user '{}': {}",
                    item.username, error
                ))
            })?;
            let model = UserDbModel {
                id: existing_user
                    .map(|user| user.id.clone())
                    .unwrap_or_else(|| item.id.clone()),
                username: item.username.clone(),
                password_hash: item.password_hash.clone(),
                email: item.email.clone(),
                roles,
                is_active: item.is_active,
                must_change_password: item.must_change_password,
                last_login_at: item.last_login_at,
                created_at: existing_user
                    .map(|user| user.created_at)
                    .unwrap_or(item.created_at),
                updated_at: item.updated_at,
            };
            import_user(tx, &model).await?;
            if existing_user.is_some() {
                stats.users_updated += 1;
            } else {
                stats.users_created += 1;
            }
        }

        if replace {
            let imported: HashSet<&str> = users.iter().map(|item| item.username.as_str()).collect();
            for (username, existing_user) in &existing {
                if !imported.contains(username.as_str()) {
                    delete_user(tx, &existing_user.id).await?;
                    stats.users_deleted += 1;
                }
            }
        }
    }

    invalidate_all_for_import(tx).await?;
    Ok(stats)
}

/// Export every persisted user account as backup [`UserExport`] entries.
pub async fn export_users(user_repo: &dyn UserRepository) -> crate::Result<Vec<UserExport>> {
    let total_users = user_repo.count().await?;
    let users = user_repo.list(total_users, 0).await?;

    Ok(users
        .into_iter()
        .map(|u: UserDbModel| {
            let roles = u.get_roles();
            UserExport {
                id: u.id,
                username: u.username,
                password_hash: u.password_hash,
                email: u.email,
                roles,
                is_active: u.is_active,
                must_change_password: u.must_change_password,
                last_login_at: u.last_login_at,
                created_at: u.created_at,
                updated_at: u.updated_at,
            }
        })
        .collect())
}
