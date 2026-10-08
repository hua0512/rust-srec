//! Account selections stored per scope.
//!
//! A selection belongs to a platform, to one template for one platform, or to
//! one streamer, and lists its accounts in order. A scope without a row
//! inherits from the next layer, so `inherit` is never stored. SQLite keeps
//! every member on the selection's platform, refuses to delete a selected
//! profile and removes the rows of a deleted owner, a streamer marked deleted
//! and a streamer that moves to another platform. Rules it cannot express are
//! checked here before a row is written: member counts per mode, profiles being
//! retired, and an owner that exists on the selection's platform. The
//! Streamlink platform serves many unrelated sites, so one account chosen at
//! its platform or template level would send that site's cookies to every
//! other site; there only a streamer may choose, and only `none` or one fixed
//! account, because Streamlink failures are never classified as account
//! failures that a pool could fail over from. A Streamlink streamer that does
//! not choose uses the account whose site covers its URL, which
//! `load_for_streamer` adds as a layer of its own.
//!
//! The HTTP API and backups carry a selection inside the configuration it
//! belongs to (`credential_selection` on a platform, inside a template's
//! platform override, or inside a streamer's specific configuration). Writes
//! take it out of the stored document here, and readers put the stored one
//! back.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::SqliteConnection;

use crate::credentials::{
    CredentialOwner, CredentialSelection, PoolStrategy, ProfileError, SelectionReference,
};
use crate::database::repositories::credential_profiles;
use crate::{Error, Result};

/// The configuration key that carries a selection on the wire.
pub const SELECTION_KEY: &str = "credential_selection";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSelection {
    pub owner: CredentialOwner,
    pub platform_id: String,
    pub platform_name: String,
    pub selection: CredentialSelection,
    /// For the account a Streamlink streamer uses through its site, that
    /// site. Such a layer is owned by the platform and never stored.
    pub site: Option<String>,
}

/// How a refused selection write names the selecting scope:
/// `platform:<id>`, `template:<id>:<platform name>` or `streamer:<id>`.
fn reference(owner: &CredentialOwner, platform_name: &str) -> String {
    match owner {
        CredentialOwner::Platform { platform_id } => format!("platform:{platform_id}"),
        CredentialOwner::Template { template_id } => {
            format!("template:{template_id}:{platform_name}")
        }
        CredentialOwner::Streamer { streamer_id } => format!("streamer:{streamer_id}"),
    }
}

fn strategy_text(strategy: PoolStrategy) -> &'static str {
    match strategy {
        PoolStrategy::Priority => "priority",
        PoolStrategy::RoundRobin => "round_robin",
    }
}

#[derive(sqlx::FromRow)]
struct SelectionRow {
    id: i64,
    platform_config_id: String,
    platform_name: String,
    template_config_id: Option<String>,
    streamer_id: Option<String>,
    mode: String,
    strategy: Option<String>,
    failover: Option<bool>,
    max_attempts: Option<i64>,
    profile_id: Option<String>,
}

const SELECT_ROWS: &str = "SELECT s.id, s.platform_config_id, p.platform_name, s.template_config_id, s.streamer_id, s.mode, s.strategy, s.failover, s.max_attempts, m.profile_id FROM credential_selections s JOIN platform_config p ON p.id = s.platform_config_id LEFT JOIN credential_selection_members m ON m.selection_id = s.id";
const ORDER_ROWS: &str = " ORDER BY s.id, m.position";

fn malformed(id: i64) -> Error {
    Error::Other(format!("stored credential selection {id} is malformed"))
}

fn decode(row: SelectionRow, members: Vec<String>) -> Result<StoredSelection> {
    let owner = match (row.template_config_id, row.streamer_id) {
        (None, None) => CredentialOwner::Platform {
            platform_id: row.platform_config_id.clone(),
        },
        (Some(template_id), None) => CredentialOwner::Template { template_id },
        (None, Some(streamer_id)) => CredentialOwner::Streamer { streamer_id },
        (Some(_), Some(_)) => return Err(malformed(row.id)),
    };
    let selection = match (row.mode.as_str(), members.len()) {
        ("none", 0) => CredentialSelection::None,
        ("fixed", 1) => CredentialSelection::Fixed {
            credential_id: members
                .into_iter()
                .next()
                .ok_or_else(|| malformed(row.id))?,
        },
        ("pool", 1..) => CredentialSelection::Pool {
            credential_ids: members,
            strategy: match row.strategy.as_deref() {
                Some("priority") => PoolStrategy::Priority,
                Some("round_robin") => PoolStrategy::RoundRobin,
                _ => return Err(malformed(row.id)),
            },
            failover: row.failover.ok_or_else(|| malformed(row.id))?,
            max_attempts: row
                .max_attempts
                .and_then(|attempts| u8::try_from(attempts).ok())
                .ok_or_else(|| malformed(row.id))?,
        },
        _ => return Err(malformed(row.id)),
    };
    selection.validate().map_err(|_| malformed(row.id))?;
    Ok(StoredSelection {
        owner,
        platform_id: row.platform_config_id,
        platform_name: row.platform_name,
        selection,
        site: None,
    })
}

/// Rows arrive ordered by selection and member position; each selection is
/// one run of rows.
fn fold(rows: Vec<SelectionRow>) -> Result<Vec<StoredSelection>> {
    let mut selections = Vec::new();
    let mut current: Option<(SelectionRow, Vec<String>)> = None;
    for mut row in rows {
        let member = row.profile_id.take();
        match current.as_mut() {
            Some((head, members)) if head.id == row.id => members.extend(member),
            _ => {
                if let Some((head, members)) = current.replace((row, member.into_iter().collect()))
                {
                    selections.push(decode(head, members)?);
                }
            }
        }
    }
    if let Some((head, members)) = current {
        selections.push(decode(head, members)?);
    }
    Ok(selections)
}

async fn fetch(
    connection: &mut SqliteConnection,
    filter: &str,
    binds: &[&str],
) -> Result<Vec<StoredSelection>> {
    let sql = format!("{SELECT_ROWS}{filter}{ORDER_ROWS}");
    let mut query = sqlx::query_as::<_, SelectionRow>(sqlx::AssertSqlSafe(sql));
    for value in binds {
        query = query.bind(*value);
    }
    fold(query.fetch_all(connection).await?)
}

/// Every stored selection.
pub(crate) async fn load_all(connection: &mut SqliteConnection) -> Result<Vec<StoredSelection>> {
    fetch(connection, "", &[]).await
}

/// The selections an owner stores: a platform's own, a template's for each
/// platform, or a streamer's.
pub(crate) async fn load_owner(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
) -> Result<Vec<StoredSelection>> {
    match owner {
        CredentialOwner::Platform { platform_id } => {
            fetch(
                connection,
                " WHERE s.platform_config_id = ? AND s.template_config_id IS NULL AND s.streamer_id IS NULL",
                &[platform_id],
            )
            .await
        }
        CredentialOwner::Template { template_id } => {
            fetch(connection, " WHERE s.template_config_id = ?", &[template_id]).await
        }
        CredentialOwner::Streamer { streamer_id } => {
            fetch(connection, " WHERE s.streamer_id = ?", &[streamer_id]).await
        }
    }
}

/// The layers a streamer resolves through, most specific first: its own
/// selection, its template's for the platform, and the platform's. On the
/// Streamlink platform the account whose site covers the streamer's URL
/// follows as a fixed selection of the platform.
pub(crate) async fn load_for_streamer(
    connection: &mut SqliteConnection,
    streamer_id: &str,
    platform_id: &str,
    template_id: Option<&str>,
) -> Result<Vec<StoredSelection>> {
    let mut layers = fetch(
        connection,
        " WHERE s.platform_config_id = ? AND (s.streamer_id = ? OR (s.template_config_id IS NOT NULL AND s.template_config_id = ?) OR (s.template_config_id IS NULL AND s.streamer_id IS NULL))",
        &[platform_id, streamer_id, template_id.unwrap_or_default()],
    )
    .await?;
    if template_id.is_none() {
        layers.retain(|stored| !matches!(stored.owner, CredentialOwner::Template { .. }));
    }
    layers.sort_by_key(|stored| stored.owner.precedence());
    layers.extend(site_layer(connection, streamer_id, platform_id).await?);
    Ok(layers)
}

async fn site_layer(
    connection: &mut SqliteConnection,
    streamer_id: &str,
    platform_id: &str,
) -> Result<Option<StoredSelection>> {
    let streamer: Option<(String, String)> = sqlx::query_as(
        "SELECT s.url, p.platform_name FROM streamers s JOIN platform_config p ON p.id = ? WHERE s.id = ?",
    )
    .bind(platform_id)
    .bind(streamer_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((url, platform_name)) = streamer else {
        return Ok(None);
    };
    if !crate::domain::is_streamlink_platform(&platform_name) {
        return Ok(None);
    }
    let Some((site, credential_id)) =
        credential_profiles::site_accounts(connection, platform_id, &url)
            .await?
            .into_iter()
            .next()
    else {
        return Ok(None);
    };
    Ok(Some(StoredSelection {
        owner: CredentialOwner::Platform {
            platform_id: platform_id.to_owned(),
        },
        platform_id: platform_id.to_owned(),
        platform_name,
        selection: CredentialSelection::Fixed { credential_id },
        site: Some(site),
    }))
}

/// The layers a selection at `owner` resolves through: a platform reads only
/// its own, a template its own then the platform's.
pub(crate) async fn load_for_owner(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
) -> Result<Vec<StoredSelection>> {
    let mut layers = load_owner(connection, owner).await?;
    layers.retain(|stored| stored.platform_id == platform_id);
    if !matches!(owner, CredentialOwner::Platform { .. }) {
        layers.extend(
            load_owner(
                connection,
                &CredentialOwner::Platform {
                    platform_id: platform_id.to_owned(),
                },
            )
            .await?,
        );
    }
    Ok(layers)
}

#[derive(sqlx::FromRow)]
struct ReferringRow {
    profile_id: String,
    platform_config_id: String,
    platform_name: String,
    template_config_id: Option<String>,
    template_name: Option<String>,
    streamer_id: Option<String>,
    streamer_name: Option<String>,
}

/// The scopes whose selections list each of `profile_ids`, named, read in one
/// query and ordered platform, templates, then streamers, each by name.
pub(crate) async fn referring(
    connection: &mut SqliteConnection,
    profile_ids: &[String],
) -> Result<Vec<(String, SelectionReference)>> {
    let rows: Vec<ReferringRow> = sqlx::query_as(
        "SELECT m.profile_id, s.platform_config_id, p.platform_name, s.template_config_id, t.name AS template_name, s.streamer_id, st.name AS streamer_name \
         FROM credential_selection_members m \
         JOIN credential_selections s ON s.id = m.selection_id \
         JOIN platform_config p ON p.id = s.platform_config_id \
         LEFT JOIN template_config t ON t.id = s.template_config_id \
         LEFT JOIN streamers st ON st.id = s.streamer_id \
         WHERE m.profile_id IN (SELECT value FROM json_each(?)) \
         ORDER BY m.profile_id, s.streamer_id IS NOT NULL, s.template_config_id IS NOT NULL, COALESCE(t.name, st.name, p.platform_name), s.id",
    )
    .bind(serde_json::to_string(profile_ids)?)
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|row| {
            let (owner, name) = match (row.template_config_id, row.streamer_id) {
                (None, None) => (
                    CredentialOwner::Platform {
                        platform_id: row.platform_config_id.clone(),
                    },
                    row.platform_name.clone(),
                ),
                (Some(template_id), None) => (
                    CredentialOwner::Template {
                        template_id: template_id.clone(),
                    },
                    row.template_name.unwrap_or(template_id),
                ),
                (None, Some(streamer_id)) => (
                    CredentialOwner::Streamer {
                        streamer_id: streamer_id.clone(),
                    },
                    row.streamer_name.unwrap_or(streamer_id),
                ),
                (Some(_), Some(_)) => {
                    return Err(Error::Other(
                        "stored credential selection has two owners".into(),
                    ));
                }
            };
            Ok((
                row.profile_id,
                SelectionReference {
                    owner,
                    name,
                    platform_id: row.platform_config_id,
                    platform_name: row.platform_name,
                },
            ))
        })
        .collect()
}

fn scope_filter(owner: &CredentialOwner, platform_id: &str) -> Result<(&'static str, Vec<String>)> {
    Ok(match owner {
        CredentialOwner::Platform {
            platform_id: owner_platform,
        } => {
            if owner_platform != platform_id {
                return Err(ProfileError::InvalidOwner.into());
            }
            (
                "platform_config_id = ? AND template_config_id IS NULL AND streamer_id IS NULL",
                vec![platform_id.to_owned()],
            )
        }
        CredentialOwner::Template { template_id } => (
            "template_config_id = ? AND platform_config_id = ?",
            vec![template_id.clone(), platform_id.to_owned()],
        ),
        CredentialOwner::Streamer { streamer_id } => ("streamer_id = ?", vec![streamer_id.clone()]),
    })
}

async fn delete_scope(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
) -> Result<bool> {
    let (filter, binds) = scope_filter(owner, platform_id)?;
    let sql = format!("DELETE FROM credential_selections WHERE {filter}");
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for value in &binds {
        query = query.bind(value);
    }
    Ok(query.execute(connection).await?.rows_affected() > 0)
}

/// Remove every selection `owner` stores, on any platform.
pub(crate) async fn clear_owner(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
) -> Result<bool> {
    let query = match owner {
        CredentialOwner::Platform { .. } => {
            "DELETE FROM credential_selections WHERE platform_config_id = ? AND template_config_id IS NULL AND streamer_id IS NULL"
        }
        CredentialOwner::Template { .. } => {
            "DELETE FROM credential_selections WHERE template_config_id = ?"
        }
        CredentialOwner::Streamer { .. } => {
            "DELETE FROM credential_selections WHERE streamer_id = ?"
        }
    };
    Ok(sqlx::query(query)
        .bind(owner.id())
        .execute(connection)
        .await?
        .rows_affected()
        > 0)
}

/// Store `selection` as `owner`'s choice on `platform_id`; `inherit` removes
/// the stored one. Returns whether anything changed. Writing the selection that
/// is already stored is a no-op, so saving a form unchanged never fails on a
/// profile that is being retired.
pub(crate) async fn set(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
    selection: &CredentialSelection,
) -> Result<bool> {
    let Some((mode, strategy, failover, max_attempts)) = stored_mode(selection) else {
        return delete_scope(connection, owner, platform_id).await;
    };
    let current = load_owner(connection, owner)
        .await?
        .into_iter()
        .find(|stored| {
            stored.platform_id == platform_id || matches!(owner, CredentialOwner::Streamer { .. })
        });
    if current
        .as_ref()
        .is_some_and(|stored| stored.platform_id == platform_id && stored.selection == *selection)
    {
        return Ok(false);
    }
    selection.validate()?;
    let platform_name = credential_profiles::require_owner(connection, owner, platform_id).await?;
    require_scope_allowed(&platform_name, owner, selection)?;
    for id in selection.profile_ids() {
        let profile_platform: Option<String> =
            sqlx::query_scalar("SELECT platform_config_id FROM credential_profiles WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
        if profile_platform.as_deref() != Some(platform_id) {
            return Err(ProfileError::InaccessibleReferences(vec![reference(
                owner,
                &platform_name,
            )])
            .into());
        }
        credential_profiles::require_not_retiring(connection, id).await?;
    }
    if current.is_some() {
        delete_scope(connection, owner, platform_id).await?;
    }
    let (template_id, streamer_id) = match owner {
        CredentialOwner::Platform { .. } => (None, None),
        CredentialOwner::Template { template_id } => (Some(template_id.as_str()), None),
        CredentialOwner::Streamer { streamer_id } => (None, Some(streamer_id.as_str())),
    };
    let id: i64 = sqlx::query_scalar("INSERT INTO credential_selections(platform_config_id, template_config_id, streamer_id, mode, strategy, failover, max_attempts) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id")
        .bind(platform_id).bind(template_id).bind(streamer_id).bind(mode).bind(strategy).bind(failover).bind(max_attempts)
        .fetch_one(&mut *connection).await?;
    for (position, profile_id) in (0_i64..).zip(selection.profile_ids()) {
        sqlx::query("INSERT INTO credential_selection_members(selection_id, platform_config_id, position, profile_id) VALUES (?, ?, ?, ?)")
            .bind(id).bind(platform_id).bind(position).bind(profile_id)
            .execute(&mut *connection).await?;
    }
    Ok(true)
}

/// Refuse a selection the platform does not take at this scope: the Streamlink
/// platform takes only a streamer's `none` or fixed account.
fn require_scope_allowed(
    platform_name: &str,
    owner: &CredentialOwner,
    selection: &CredentialSelection,
) -> Result<()> {
    if !crate::domain::is_streamlink_platform(platform_name) {
        return Ok(());
    }
    if !matches!(owner, CredentialOwner::Streamer { .. }) {
        return Err(ProfileError::PerStreamerOnly(
            "Streamlink serves many unrelated sites, so its accounts are chosen by the sites they name or on each streamer, not on the platform or a template",
        )
        .into());
    }
    if matches!(selection, CredentialSelection::Pool { .. }) {
        return Err(ProfileError::PerStreamerOnly(
            "A Streamlink streamer uses one fixed account; account pools are not available",
        )
        .into());
    }
    Ok(())
}

type StoredMode = (
    &'static str,
    Option<&'static str>,
    Option<bool>,
    Option<i64>,
);

/// Column values for a selection that is stored; `inherit` is not.
fn stored_mode(selection: &CredentialSelection) -> Option<StoredMode> {
    match selection {
        CredentialSelection::Inherit => None,
        CredentialSelection::None => Some(("none", None, None, None)),
        CredentialSelection::Fixed { .. } => Some(("fixed", None, None, None)),
        CredentialSelection::Pool {
            strategy,
            failover,
            max_attempts,
            ..
        } => Some((
            "pool",
            Some(strategy_text(*strategy)),
            Some(*failover),
            Some(i64::from(*max_attempts)),
        )),
    }
}

/// Give `target` every selection `source` stores.
pub(crate) async fn copy_owner(
    connection: &mut SqliteConnection,
    source: &CredentialOwner,
    target: &CredentialOwner,
) -> Result<()> {
    for stored in load_owner(connection, source).await? {
        set(connection, target, &stored.platform_id, &stored.selection).await?;
    }
    Ok(())
}

/// Take the selection out of a stored configuration document, such as a
/// streamer's specific configuration. Text that is not a JSON object is left
/// as written. The document is rewritten only when it carried a selection.
pub(crate) fn take_document(raw: &mut Option<String>) -> Result<Option<CredentialSelection>> {
    let Some(mut document) = raw
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return Ok(None);
    };
    let Some(selection) = document
        .as_object_mut()
        .and_then(|fields| fields.remove(SELECTION_KEY))
    else {
        return Ok(None);
    };
    *raw = Some(document.to_string());
    CredentialSelection::from_value(selection).map(Some)
}

/// Take each platform override's selection out of a template's
/// `platform_overrides`, keyed by the override's platform name.
pub(crate) fn take_overrides(
    raw: &mut Option<String>,
) -> Result<Vec<(String, CredentialSelection)>> {
    let Some(mut overrides) = raw
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return Ok(Vec::new());
    };
    let Some(entries) = overrides.as_object_mut() else {
        return Ok(Vec::new());
    };
    let mut selections = Vec::new();
    for (platform, entry) in entries.iter_mut() {
        if let Some(selection) = entry
            .as_object_mut()
            .and_then(|fields| fields.remove(SELECTION_KEY))
        {
            selections.push((
                platform.clone(),
                CredentialSelection::from_value(selection)?,
            ));
        }
    }
    if !selections.is_empty() {
        *raw = Some(overrides.to_string());
    }
    Ok(selections)
}

/// Put the stored selection into a configuration document for a reader,
/// replacing any the document carries.
pub fn inject_document(
    document: Option<Value>,
    selection: Option<&CredentialSelection>,
) -> Option<Value> {
    let mut document = document.filter(|document| !document.is_null());
    if let Some(fields) = document.as_mut().and_then(Value::as_object_mut) {
        fields.remove(SELECTION_KEY);
    }
    let Some(selection) = selection.and_then(|selection| serde_json::to_value(selection).ok())
    else {
        return document;
    };
    let mut document = document.unwrap_or_else(|| Value::Object(Default::default()));
    if let Some(fields) = document.as_object_mut() {
        fields.insert(SELECTION_KEY.to_owned(), selection);
    }
    Some(document)
}

/// Put a template's stored selections into its platform overrides, keyed by
/// platform name, replacing any the overrides carry.
pub fn inject_overrides(overrides: Option<Value>, selections: &[StoredSelection]) -> Option<Value> {
    let mut overrides = overrides.filter(|overrides| !overrides.is_null());
    if let Some(entries) = overrides.as_mut().and_then(Value::as_object_mut) {
        for entry in entries.values_mut() {
            if let Some(fields) = entry.as_object_mut() {
                fields.remove(SELECTION_KEY);
            }
        }
    }
    if selections.is_empty() {
        return overrides;
    }
    let mut overrides = overrides.unwrap_or_else(|| Value::Object(Default::default()));
    if let Some(entries) = overrides.as_object_mut() {
        for stored in selections {
            let Ok(selection) = serde_json::to_value(&stored.selection) else {
                continue;
            };
            let entry = entries
                .entry(stored.platform_name.clone())
                .or_insert_with(|| Value::Object(Default::default()));
            if !entry.is_object() {
                *entry = Value::Object(Default::default());
            }
            if let Some(fields) = entry.as_object_mut() {
                fields.insert(SELECTION_KEY.to_owned(), selection);
            }
        }
    }
    Some(overrides)
}

/// Apply the selections a template write carried, keyed by platform name.
/// Platforms the write did not mention keep their selection.
pub(crate) async fn write_template(
    connection: &mut SqliteConnection,
    template_id: &str,
    selections: Vec<(String, CredentialSelection)>,
) -> Result<()> {
    if selections.is_empty() {
        return Ok(());
    }
    let platforms: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT platform_name, id FROM platform_config")
            .fetch_all(&mut *connection)
            .await?
            .into_iter()
            .collect();
    let owner = CredentialOwner::Template {
        template_id: template_id.to_owned(),
    };
    for (name, selection) in selections {
        let Some(platform_id) = platforms.get(&name) else {
            let mut names: Vec<&str> = platforms.keys().map(String::as_str).collect();
            names.sort_unstable();
            return Err(Error::validation(format!(
                "managed template override must use a canonical platform name; available names: {}",
                names.join(", ")
            )));
        };
        set(connection, &owner, platform_id, &selection).await?;
    }
    Ok(())
}

/// Apply the selection a streamer write carried. `previous` is the streamer's
/// platform and selection before the write. A streamer that moves to another
/// platform inherits there: SQLite drops its old selection, and a request that
/// merely repeats that old selection does not restore it.
pub(crate) async fn write_streamer(
    connection: &mut SqliteConnection,
    streamer_id: &str,
    platform_id: &str,
    incoming: Option<CredentialSelection>,
    previous: Option<(String, Option<CredentialSelection>)>,
) -> Result<bool> {
    let moved = previous
        .as_ref()
        .is_some_and(|(previous_platform, _)| previous_platform != platform_id);
    let dropped = moved
        && previous
            .as_ref()
            .is_some_and(|(_, selection)| selection.is_some());
    let Some(incoming) = incoming else {
        return Ok(dropped);
    };
    if moved
        && previous
            .as_ref()
            .and_then(|(_, selection)| selection.as_ref())
            == Some(&incoming)
    {
        return Ok(dropped);
    }
    let owner = CredentialOwner::Streamer {
        streamer_id: streamer_id.to_owned(),
    };
    Ok(set(connection, &owner, platform_id, &incoming).await? || dropped)
}

/// The platform and selection a streamer has before a write changes it.
pub(crate) async fn streamer_before_write(
    connection: &mut SqliteConnection,
    streamer_id: &str,
) -> Result<Option<(String, Option<CredentialSelection>)>> {
    let platform: Option<String> =
        sqlx::query_scalar("SELECT platform_config_id FROM streamers WHERE id = ?")
            .bind(streamer_id)
            .fetch_optional(&mut *connection)
            .await?;
    let Some(platform) = platform else {
        return Ok(None);
    };
    let selection = load_owner(
        connection,
        &CredentialOwner::Streamer {
            streamer_id: streamer_id.to_owned(),
        },
    )
    .await?
    .into_iter()
    .next()
    .map(|stored| stored.selection);
    Ok(Some((platform, selection)))
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// The selection `owner` stores for `platform_id`; `None` inherits.
    pub(crate) async fn load_scope(
        connection: &mut SqliteConnection,
        owner: &CredentialOwner,
        platform_id: &str,
    ) -> Result<Option<CredentialSelection>> {
        Ok(load_owner(connection, owner)
            .await?
            .into_iter()
            .find(|stored| stored.platform_id == platform_id)
            .map(|stored| stored.selection))
    }

    /// Store a selection outside any configuration write.
    async fn set_selection(
        pool: &sqlx::SqlitePool,
        owner: &CredentialOwner,
        platform_id: &str,
        selection: &CredentialSelection,
    ) {
        let mut connection = pool.acquire().await.expect("test connection");
        set(&mut connection, owner, platform_id, selection)
            .await
            .expect("test selection");
    }

    /// Store a platform's own selection.
    pub(crate) async fn set_platform_selection(
        pool: &sqlx::SqlitePool,
        platform_id: &str,
        selection: &CredentialSelection,
    ) {
        set_selection(
            pool,
            &CredentialOwner::Platform {
                platform_id: platform_id.to_owned(),
            },
            platform_id,
            selection,
        )
        .await;
    }
}

#[cfg(test)]
mod tests;
