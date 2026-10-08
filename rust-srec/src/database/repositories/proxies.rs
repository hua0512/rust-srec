//! Saved proxies and the routes that name them.
//!
//! Every function takes a connection whose transaction the caller owns, so a
//! route is written in the same transaction as the scope it belongs to.

use std::collections::HashMap;

use sqlx::SqliteConnection;

use crate::proxies::{
    AccountReference, NamedReference, ProxyEndpoint, ProxyEntry, ProxyError, ProxyReferences,
    ProxyRoute, ResolvedRoute, RouteSource, SystemProxy, TemplateReference, resolve,
};
use crate::{Error, Result};

const MAX_NAME_CHARS: usize = 64;

/// The key a streamer's configuration document carries its route under.
pub const ROUTE_KEY: &str = "proxy_route";
/// The key legacy documents and backups carry JSON proxy settings under:
/// conversion reads it and writes refuse it.
pub const LEGACY_KEY: &str = "proxy_config";

/// A scope or account that stores a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteOwner {
    Global,
    Platform(String),
    Template(String),
    Streamer(String),
    Account(String),
}

impl RouteOwner {
    fn table(&self) -> &'static str {
        match self {
            Self::Global => "global_config",
            Self::Platform(_) => "platform_config",
            Self::Template(_) => "template_config",
            Self::Streamer(_) => "streamers",
            Self::Account(_) => "credential_profiles",
        }
    }

    fn id(&self) -> Option<&str> {
        match self {
            Self::Global => None,
            Self::Platform(id) | Self::Template(id) | Self::Streamer(id) | Self::Account(id) => {
                Some(id)
            }
        }
    }
}

/// Changes to an entry; omitted fields keep their value. `Debug` never
/// shows the password.
#[derive(Default, Clone)]
pub struct ProxyUpdate {
    pub name: Option<String>,
    pub url: Option<String>,
    /// `Some(None)` removes the login, password included.
    pub username: Option<Option<String>>,
    /// Omitted keeps the saved password.
    pub password: Option<String>,
}

impl std::fmt::Debug for ProxyUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyUpdate")
            .field("name", &self.name)
            .field(
                "url",
                &self
                    .url
                    .as_deref()
                    .map(platforms_parser::proxy::redacted_url),
            )
            .field("username", &self.username.as_ref().map(Option::is_some))
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// The outcome of an entry update.
#[derive(Debug)]
pub struct UpdatedProxy {
    pub entry: ProxyEntry,
    /// The address or username changed: the entry now reaches another exit.
    pub exit_changed: bool,
    /// Platforms owning accounts pinned to the entry, whose revisions moved.
    pub pinned_platforms: Vec<String>,
}

fn validate_name(name: &str) -> std::result::Result<String, ProxyError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(ProxyError::invalid(format!(
            "proxy name must contain 1..{MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(ProxyError::invalid(
            "proxy name must not contain control characters",
        ));
    }
    Ok(name.to_owned())
}

pub(crate) async fn list(connection: &mut SqliteConnection) -> Result<Vec<ProxyEntry>> {
    Ok(
        sqlx::query_as::<_, ProxyEntry>("SELECT * FROM proxies ORDER BY name COLLATE NOCASE, id")
            .fetch_all(connection)
            .await?,
    )
}

pub(crate) async fn find(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ProxyEntry>> {
    Ok(
        sqlx::query_as::<_, ProxyEntry>("SELECT * FROM proxies WHERE id = ?")
            .bind(id)
            .fetch_optional(connection)
            .await?,
    )
}

pub(crate) async fn get(connection: &mut SqliteConnection, id: &str) -> Result<ProxyEntry> {
    find(connection, id)
        .await?
        .ok_or_else(|| Error::not_found("Proxy", id))
}

pub(crate) async fn find_by_name(
    connection: &mut SqliteConnection,
    name: &str,
) -> Result<Option<ProxyEntry>> {
    Ok(
        sqlx::query_as::<_, ProxyEntry>("SELECT * FROM proxies WHERE name = ? COLLATE NOCASE")
            .bind(name.trim())
            .fetch_optional(connection)
            .await?,
    )
}

/// The entry reaching the same exit as `endpoint`, if any.
pub(crate) async fn find_by_exit(
    connection: &mut SqliteConnection,
    endpoint: &ProxyEndpoint,
) -> Result<Option<ProxyEntry>> {
    Ok(sqlx::query_as::<_, ProxyEntry>(
        "SELECT * FROM proxies WHERE url = ? AND COALESCE(username, '') = COALESCE(?, '')",
    )
    .bind(&endpoint.url)
    .bind(&endpoint.username)
    .fetch_optional(connection)
    .await?)
}

/// Rejects a name or exit another entry already has.
async fn require_unique(
    connection: &mut SqliteConnection,
    id: Option<&str>,
    name: &str,
    endpoint: &ProxyEndpoint,
) -> Result<()> {
    if let Some(other) = find_by_name(connection, name).await?
        && Some(other.id.as_str()) != id
    {
        return Err(ProxyError::NameTaken(other.name).into());
    }
    if let Some(other) = find_by_exit(connection, endpoint).await?
        && Some(other.id.as_str()) != id
    {
        return Err(ProxyError::DuplicateEndpoint { name: other.name }.into());
    }
    Ok(())
}

/// Saves a new entry under `id`, or a fresh ID when `None`.
pub(crate) async fn create(
    connection: &mut SqliteConnection,
    id: Option<&str>,
    name: &str,
    endpoint: &ProxyEndpoint,
) -> Result<ProxyEntry> {
    let name = validate_name(name)?;
    let endpoint = crate::proxies::canonical(endpoint)?;
    require_unique(connection, None, &name, &endpoint).await?;
    let id = id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let now = crate::database::time::now_ms();
    sqlx::query("INSERT INTO proxies(id, name, url, username, password, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(&name)
        .bind(&endpoint.url)
        .bind(&endpoint.username)
        .bind(&endpoint.password)
        .bind(now)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    get(connection, &id).await
}

/// Applies `update` when the entry is still at `expected_version`. A new
/// address or username moves every account pinned to the entry to a new
/// revision with unknown health: the account now reaches the platform from
/// another address.
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: &str,
    expected_version: i64,
    update: ProxyUpdate,
) -> Result<UpdatedProxy> {
    let current = get(connection, id).await?;
    if current.version != expected_version {
        return Err(ProxyError::StaleVersion.into());
    }
    let name = validate_name(update.name.as_deref().unwrap_or(&current.name))?;
    let username = match update.username {
        Some(username) => username.filter(|username| !username.is_empty()),
        None => current.username.clone(),
    };
    let password = match (&username, update.password) {
        (None, _) => None,
        (Some(_), Some(password)) => Some(password),
        (Some(_), None) => current.password.clone(),
    };
    let endpoint = crate::proxies::canonical(&ProxyEndpoint {
        url: update.url.unwrap_or_else(|| current.url.clone()),
        username,
        password,
    })?;
    require_unique(connection, Some(id), &name, &endpoint).await?;
    let exit_changed = endpoint.url != current.url || endpoint.username != current.username;
    let now = crate::database::time::now_ms();
    sqlx::query("UPDATE proxies SET name = ?, url = ?, username = ?, password = ?, version = version + 1, updated_at = ? WHERE id = ? AND version = ?")
        .bind(&name)
        .bind(&endpoint.url)
        .bind(&endpoint.username)
        .bind(&endpoint.password)
        .bind(now)
        .bind(id)
        .bind(expected_version)
        .execute(&mut *connection)
        .await?;
    let pinned_platforms = if exit_changed {
        pin_moved(connection, id, now).await?
    } else {
        Vec::new()
    };
    Ok(UpdatedProxy {
        entry: get(connection, id).await?,
        exit_changed,
        pinned_platforms,
    })
}

/// Starts a new revision of every account pinned to the entry and drops its
/// health. Returns the platforms owning those accounts.
pub(crate) async fn pin_moved(
    connection: &mut SqliteConnection,
    id: &str,
    now: i64,
) -> Result<Vec<String>> {
    sqlx::query("DELETE FROM credential_profile_health WHERE profile_id IN (SELECT id FROM credential_profiles WHERE proxy_id = ?)")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query(
        "UPDATE credential_profiles SET revision = revision + 1, updated_at = ? WHERE proxy_id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *connection)
    .await?;
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT platform_config_id FROM credential_profiles WHERE proxy_id = ? ORDER BY 1",
    )
    .bind(id)
    .fetch_all(connection)
    .await?)
}

/// Deletes an unused entry. `expected_version` guards against deleting an
/// entry edited since it was read.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: &str,
    expected_version: Option<i64>,
) -> Result<()> {
    let current = get(connection, id).await?;
    if expected_version.is_some_and(|version| version != current.version) {
        return Err(ProxyError::StaleVersion.into());
    }
    let references = references_of(connection, id).await?;
    if !references.is_empty() {
        return Err(ProxyError::Referenced(Box::new(references)).into());
    }
    match sqlx::query("DELETE FROM proxies WHERE id = ?")
        .bind(id)
        .execute(&mut *connection)
        .await
    {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
            Err(ProxyError::Referenced(Box::new(references_of(connection, id).await?)).into())
        }
        Err(error) => Err(error.into()),
    }
}

/// The routes naming the entry. Retired streamers do not hold one.
pub(crate) async fn references_of(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<ProxyReferences> {
    let global: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM global_config WHERE proxy_id = ?)")
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
    let platforms = sqlx::query_as::<_, (String, String)>(
        "SELECT id, platform_name FROM platform_config WHERE proxy_id = ? ORDER BY platform_name, id",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name)| NamedReference { id, name })
    .collect();
    let templates = sqlx::query_as::<_, (String, String, bool)>(
        "SELECT t.id, t.name, EXISTS(SELECT 1 FROM retirement_config_deletions r WHERE r.kind = 'template' AND r.config_id = t.id) FROM template_config t WHERE t.proxy_id = ? ORDER BY t.name, t.id",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name, being_removed)| TemplateReference {
        id,
        name,
        being_removed,
    })
    .collect();
    let streamers = sqlx::query_as::<_, (String, String)>(
        "SELECT id, name FROM streamers WHERE proxy_id = ? AND deleted_at IS NULL ORDER BY name, id",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name)| NamedReference { id, name })
    .collect();
    let accounts = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT p.id, p.label, p.platform_config_id, c.platform_name FROM credential_profiles p JOIN platform_config c ON c.id = p.platform_config_id WHERE p.proxy_id = ? ORDER BY c.platform_name, p.label, p.id",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, label, platform_id, platform_name)| AccountReference {
        id,
        label,
        platform_id,
        platform_name,
    })
    .collect();
    Ok(ProxyReferences {
        global,
        platforms,
        templates,
        streamers,
        accounts,
    })
}

/// How many routes name each entry.
pub(crate) async fn usage_counts(
    connection: &mut SqliteConnection,
) -> Result<HashMap<String, usize>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT proxy_id, COUNT(*) FROM (
            SELECT proxy_id FROM global_config WHERE proxy_id IS NOT NULL
            UNION ALL SELECT proxy_id FROM platform_config WHERE proxy_id IS NOT NULL
            UNION ALL SELECT proxy_id FROM template_config WHERE proxy_id IS NOT NULL
            UNION ALL SELECT proxy_id FROM streamers WHERE proxy_id IS NOT NULL AND deleted_at IS NULL
            UNION ALL SELECT proxy_id FROM credential_profiles WHERE proxy_id IS NOT NULL
        ) GROUP BY proxy_id",
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, count)| (id, usize::try_from(count).unwrap_or(usize::MAX)))
        .collect())
}

/// The route `owner` stores. A missing owner is not found.
pub(crate) async fn route_of(
    connection: &mut SqliteConnection,
    owner: &RouteOwner,
) -> Result<ProxyRoute> {
    let query = match owner.id() {
        Some(_) => format!(
            "SELECT proxy_route, proxy_id FROM {} WHERE id = ?",
            owner.table()
        ),
        None => format!(
            "SELECT proxy_route, proxy_id FROM {} ORDER BY rowid LIMIT 1",
            owner.table()
        ),
    };
    let mut statement = sqlx::query_as::<_, (String, Option<String>)>(sqlx::AssertSqlSafe(query));
    if let Some(id) = owner.id() {
        statement = statement.bind(id);
    }
    let (kind, id) = statement
        .fetch_optional(connection)
        .await?
        .ok_or_else(|| Error::not_found(owner.table(), owner.id().unwrap_or("global")))?;
    Ok(ProxyRoute::from_columns(&kind, id.as_deref())?)
}

/// Every stored route of one kind of scope, by scope ID.
pub(crate) async fn routes_of_kind(
    connection: &mut SqliteConnection,
    owner_kind: &RouteOwner,
) -> Result<HashMap<String, ProxyRoute>> {
    let query = format!(
        "SELECT id, proxy_route, proxy_id FROM {} WHERE proxy_route != 'inherit'",
        owner_kind.table()
    );
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(query))
        .fetch_all(connection)
        .await?;
    rows.into_iter()
        .map(|(id, kind, proxy)| Ok((id, ProxyRoute::from_columns(&kind, proxy.as_deref())?)))
        .collect()
}

/// Stores `route` on `owner`. The global route cannot inherit, and a route
/// must name an existing entry.
pub(crate) async fn set_route(
    connection: &mut SqliteConnection,
    owner: &RouteOwner,
    route: &ProxyRoute,
) -> Result<()> {
    if *owner == RouteOwner::Global && route.is_inherit() {
        return Err(ProxyError::GlobalInherit.into());
    }
    if let Some(id) = route.proxy_id()
        && find(connection, id).await?.is_none()
    {
        return Err(ProxyError::Missing(id.to_owned()).into());
    }
    let query = match owner.id() {
        Some(_) => format!(
            "UPDATE {} SET proxy_route = ?, proxy_id = ? WHERE id = ?",
            owner.table()
        ),
        None => format!("UPDATE {} SET proxy_route = ?, proxy_id = ?", owner.table()),
    };
    let mut statement = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(route.kind())
        .bind(route.proxy_id());
    if let Some(id) = owner.id() {
        statement = statement.bind(id);
    }
    let written = statement.execute(connection).await?.rows_affected();
    if written == 0 {
        return Err(Error::not_found(
            owner.table(),
            owner.id().unwrap_or("global"),
        ));
    }
    Ok(())
}

/// The connection of the first route that does not inherit, loading the
/// entry it names.
pub(crate) async fn resolve_layers(
    connection: &mut SqliteConnection,
    layers: &[(RouteSource, ProxyRoute)],
    system: &SystemProxy,
) -> Result<ResolvedRoute> {
    // Only the deciding layer's entry is ever read.
    let named = layers
        .iter()
        .find(|(_, route)| !route.is_inherit())
        .and_then(|(_, route)| route.proxy_id());
    let entry = match named {
        Some(id) => find(connection, id).await?,
        None => None,
    };
    Ok(resolve(
        layers,
        |id| entry.as_ref().filter(|entry| entry.id == id),
        system,
    )?)
}

/// A streamer's route: its own, then its template's, then its platform's,
/// then the global route.
pub(crate) async fn resolve_streamer(
    connection: &mut SqliteConnection,
    streamer_id: &str,
    platform_id: &str,
    template_id: Option<&str>,
    system: &SystemProxy,
) -> Result<ResolvedRoute> {
    let mut layers = Vec::with_capacity(4);
    // A streamer being created has no row yet and inherits.
    let streamer = match route_of(connection, &RouteOwner::Streamer(streamer_id.to_owned())).await {
        Ok(route) => route,
        Err(Error::NotFound { .. }) => ProxyRoute::Inherit,
        Err(error) => return Err(error),
    };
    layers.push((RouteSource::Streamer, streamer));
    if let Some(template_id) = template_id {
        layers.push((
            RouteSource::Template,
            route_of(connection, &RouteOwner::Template(template_id.to_owned())).await?,
        ));
    }
    layers.extend(scope_layers(connection, Some(platform_id)).await?);
    resolve_layers(connection, &layers, system).await
}

/// A platform's route then the global route; only the global route when the
/// platform is unknown.
pub(crate) async fn scope_layers(
    connection: &mut SqliteConnection,
    platform_id: Option<&str>,
) -> Result<Vec<(RouteSource, ProxyRoute)>> {
    let mut layers = Vec::with_capacity(2);
    if let Some(platform_id) = platform_id {
        layers.push((
            RouteSource::Platform,
            route_of(connection, &RouteOwner::Platform(platform_id.to_owned())).await?,
        ));
    }
    layers.push((
        RouteSource::Global,
        route_of(connection, &RouteOwner::Global).await?,
    ));
    Ok(layers)
}

/// The route requests of an account take: its own when it has one;
/// otherwise the operation's, or for management of the account itself
/// (checks, refreshes, sign-in) its platform's then the global route.
pub(crate) async fn resolve_account(
    connection: &mut SqliteConnection,
    platform_id: &str,
    own: &ProxyRoute,
    operation: Option<&ResolvedRoute>,
    system: &SystemProxy,
) -> Result<ResolvedRoute> {
    if !own.is_inherit() {
        return resolve_layers(connection, &[(RouteSource::Account, own.clone())], system).await;
    }
    if let Some(operation) = operation {
        return Ok(operation.clone());
    }
    let layers = scope_layers(connection, Some(platform_id)).await?;
    resolve_layers(connection, &layers, system).await
}

/// Refuses a configuration document that still sets `proxy_config`, which
/// nothing reads: a client sending it would silently lose the proxy.
pub(crate) fn reject_legacy_document(raw: Option<&str>) -> Result<()> {
    let carries = raw
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|document| {
            document
                .get(LEGACY_KEY)
                .is_some_and(|value| !value.is_null())
        });
    if carries {
        return Err(ProxyError::ConfigReplaced.into());
    }
    Ok(())
}

/// Refuses template platform overrides that set `proxy_config`.
pub(crate) fn reject_legacy_overrides(raw: Option<&str>) -> Result<()> {
    let overrides = raw.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    if overrides
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|entries| {
            entries
                .values()
                .any(|entry| entry.get(LEGACY_KEY).is_some_and(|value| !value.is_null()))
        })
    {
        return Err(ProxyError::ConfigReplaced.into());
    }
    Ok(())
}

/// Takes the route out of a stored configuration document, such as a
/// streamer's specific configuration; text that is not a JSON object is left
/// as written. The document is rewritten only when it carried a route.
pub(crate) fn take_document(raw: &mut Option<String>) -> Result<Option<ProxyRoute>> {
    let Some(mut document) = raw
        .as_deref()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
    else {
        return Ok(None);
    };
    let Some(route) = document
        .as_object_mut()
        .and_then(|fields| fields.remove(ROUTE_KEY))
    else {
        return Ok(None);
    };
    *raw = Some(document.to_string());
    if route.is_null() {
        return Ok(None);
    }
    serde_json::from_value(route).map(Some).map_err(|_| {
        ProxyError::invalid("proxy_route must be inherit, direct, system or a saved proxy").into()
    })
}

/// Puts the stored route into a configuration document for a reader,
/// replacing any the document carries; an inheriting route is left out.
pub fn inject_document(
    document: Option<serde_json::Value>,
    route: Option<&ProxyRoute>,
) -> Option<serde_json::Value> {
    let mut document = document.filter(|document| !document.is_null());
    if let Some(fields) = document.as_mut().and_then(serde_json::Value::as_object_mut) {
        fields.remove(ROUTE_KEY);
    }
    let Some(route) = route
        .filter(|route| !route.is_inherit())
        .and_then(|route| serde_json::to_value(route).ok())
    else {
        return document;
    };
    let mut document = document.unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    if let Some(fields) = document.as_object_mut() {
        fields.insert(ROUTE_KEY.to_owned(), route);
    }
    Some(document)
}

/// Saves an entry for a test and returns its ID.
#[cfg(test)]
pub(crate) async fn save_for_test(pool: &sqlx::SqlitePool, name: &str, url: &str) -> String {
    create(
        &mut pool.acquire().await.expect("test connection"),
        None,
        name,
        &ProxyEndpoint::new(url, None),
    )
    .await
    .expect("test proxy saved")
    .id
}

#[cfg(test)]
mod tests;
