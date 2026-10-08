//! Conversion of configuration-embedded cookies into credential profiles.
//!
//! Earlier releases stored cookies, tokens and account login fields directly on
//! platform, template and streamer configuration. Profiles replace them. The
//! schema migration that installs `legacy_credential_upgrade_pending` marks the
//! database conversion as owed; [`run`] performs it and drops the marker in the
//! same transaction, so an interrupted start retries the whole conversion.
//! Imported backups from those releases go through [`upgrade_bundle`]. The
//! `cookies` columns are gone; the migration that dropped them moved their
//! values into `legacy_cookies`, which this conversion reads and drops.
//!
//! Every scope that carried its own material gets a fixed selection of a
//! platform-owned profile holding exactly what its streamers used before:
//! cookies and tokens from the nearest layer with nonblank cookies, the
//! platform's SOOP login, and account extras merged down the chain. Identical
//! bundles on one platform share a profile. Blank cookie fields are unset. A
//! template's top-level cookie converts only for platforms whose streamers use
//! the template. A Streamlink account is never shared by every site, so a
//! converted Streamlink platform or template account becomes the fixed
//! selection of each Streamlink streamer that used it instead.

use std::collections::{HashMap, HashSet};

use platforms_parser::extractor::platform_configs::merge_platform_extras;
use serde_json::Value;
use sqlx::{SqliteConnection, SqlitePool};
use tracing::{info, warn};

use crate::Result;
use crate::config::backup::{ConfigExport, CredentialProfileExport, StreamerExport};
use crate::credentials::{
    CredentialMaterial, CredentialOwner, CredentialSelection, merge_cookie_updates,
};
use crate::database::repositories::{credential_profiles, credential_selections};

const MARKER: &str = "legacy_credential_upgrade_pending";
const STASH: &str = "legacy_cookies";
const LABEL_SUFFIX: &str = " (migrated)";

/// Account fields that the extractors read from configuration extras: a
/// scope that sets one owns an account even without cookies. Narrower than
/// [`crate::credentials::AUTHENTICATION_FIELDS`], which also lists fields
/// that are removed but never made a scope own an account.
const ACCOUNT_EXTRAS: [&str; 3] = ["oauth_token", "ttwid", "device_id"];

pub(crate) async fn run(pool: &SqlitePool) -> Result<()> {
    let converted =
        crate::database::run_marked_conversion(pool, MARKER, STASH, async |connection| {
            convert_database(connection).await
        })
        .await?;
    if let Some((profiles, scopes)) = converted.filter(|(_, scopes)| *scopes > 0) {
        info!(
            profiles,
            scopes, "Converted configured cookies into credential profiles"
        );
    }
    Ok(())
}

/// Convert a backup written before profiles existed, or one that still carries
/// configuration cookies on scopes without a selection, and move any Streamlink
/// platform or template selection onto the Streamlink streamers.
///
/// `keeps_stored` reports a streamer whose stored selection an import keeps
/// when the bundle omits one, as a merge does. Such a streamer gets no
/// selection moved down from its platform or template.
pub(crate) fn upgrade_bundle(
    export: &mut ConfigExport,
    keeps_stored: impl Fn(&StreamerExport) -> bool,
) -> Result<()> {
    let kept: HashSet<String> = export
        .streamers
        .iter()
        .filter(|streamer| {
            streamer
                .streamer_specific_config
                .as_ref()
                .and_then(|config| config.get("credential_selection"))
                .is_none()
                && keeps_stored(streamer)
        })
        .map(|streamer| streamer.url.clone())
        .collect();
    let platforms: Vec<LegacyPlatform> = export
        .platforms
        .iter()
        .map(|platform| LegacyPlatform {
            key: platform.platform_name.clone(),
            name: platform.platform_name.clone(),
            cookies: platform.cookies.clone(),
            fields: object_value(platform.platform_specific_config.clone()),
            selected: platform.credential_selection.is_some(),
        })
        .collect();
    let templates: Vec<LegacyTemplate> = export
        .templates
        .iter()
        .map(|template| LegacyTemplate {
            key: template.name.clone(),
            name: template.name.clone(),
            cookies: template.cookies.clone(),
            overrides: object_value(template.platform_overrides.clone()),
            retiring: false,
        })
        .collect();
    let streamers: Vec<LegacyStreamer> = export
        .streamers
        .iter()
        .map(|streamer| LegacyStreamer {
            key: streamer.url.clone(),
            name: streamer.name.clone(),
            platform_key: streamer.platform.clone(),
            template_key: streamer.template.clone(),
            config: object_value(streamer.streamer_specific_config.clone()),
            deleted: false,
            keeps_stored_selection: kept.contains(&streamer.url),
        })
        .collect();
    let plan = plan(&platforms, &templates, &streamers);
    let ids: Vec<String> = plan
        .profiles
        .iter()
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect();
    for (profile, id) in plan.profiles.iter().zip(&ids) {
        export.credential_profiles.push(CredentialProfileExport {
            id: id.clone(),
            platform: profile.platform_key.clone(),
            label: profile.label.clone(),
            enabled: true,
            cookies: profile.material.cookies.clone(),
            refresh_token: profile.material.refresh_token.clone(),
            access_token: profile.material.access_token.clone(),
            reauth_config: profile.material.reauth_config.clone(),
            proxy_route: None,
            sites: None,
        });
    }
    for platform in &mut export.platforms {
        platform.cookies = None;
        if let Some(fields) = &mut platform.platform_specific_config {
            clear_authentication(&platform.platform_name, fields);
        }
        if let Some(index) = plan.platforms.get(&platform.platform_name) {
            platform.credential_selection = Some(fixed(&ids[*index]));
        }
    }
    for template in &mut export.templates {
        template.cookies = None;
        let mut overrides = object_value(template.platform_overrides.take());
        strip_overrides(&mut overrides, &template.name, &plan, &ids)?;
        template.platform_overrides = Some(overrides);
    }
    let platform_names: HashMap<String, String> = platforms
        .iter()
        .map(|platform| (platform.key.clone(), platform.name.clone()))
        .collect();
    for streamer in &mut export.streamers {
        let mut config = object_value(streamer.streamer_specific_config.take());
        strip_streamer(
            &mut config,
            platform_names
                .get(&streamer.platform)
                .map_or("", String::as_str),
            plan.streamers.get(&streamer.url).map(|index| &ids[*index]),
        )?;
        streamer.streamer_specific_config = Some(config);
    }
    localize_streamlink_selections(export, &kept)?;
    // The schema version stays as written: other fields, such as filter
    // timezones, are read according to it.
    Ok(())
}

struct LegacyPlatform {
    key: String,
    name: String,
    cookies: Option<String>,
    fields: Value,
    selected: bool,
}

struct LegacyTemplate {
    key: String,
    name: String,
    cookies: Option<String>,
    overrides: Value,
    retiring: bool,
}

struct LegacyStreamer {
    key: String,
    name: String,
    platform_key: String,
    template_key: Option<String>,
    config: Value,
    deleted: bool,
    /// Already selects outside the configuration, and keeps that selection.
    keeps_stored_selection: bool,
}

struct NewProfile {
    platform_key: String,
    label: String,
    material: CredentialMaterial,
}

/// Profiles to create, and which one each converted scope selects.
#[derive(Default)]
struct Plan {
    profiles: Vec<NewProfile>,
    platforms: HashMap<String, usize>,
    /// Keyed by template key and platform name, as overrides are.
    templates: HashMap<(String, String), usize>,
    streamers: HashMap<String, usize>,
}

/// One configuration level as the earlier credential walk saw it.
#[derive(Clone, Copy)]
struct Layer<'a> {
    cookies: Option<&'a str>,
    tokens: Option<&'a Value>,
    extras: Option<&'a Value>,
    selected: bool,
}

impl Layer<'_> {
    fn owns_material(&self) -> bool {
        self.cookies.is_some()
            || self.extras.is_some_and(|extras| {
                ACCOUNT_EXTRAS
                    .iter()
                    .any(|key| string(Some(extras), key).is_some())
            })
    }
}

fn object_value(value: Option<Value>) -> Value {
    value
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

fn object(raw: Option<String>) -> Value {
    object_value(
        raw.as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
    )
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

fn string<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a str> {
    nonblank(
        value
            .and_then(|value| value.get(key))
            .and_then(Value::as_str),
    )
}

/// Template wrappers carry extractor fields either flat or nested; the earlier
/// walk flattened them before overlaying.
fn template_extras(entry: Option<&Value>) -> Option<Value> {
    let mut entry = entry?.clone();
    if let Some(fields) = entry.as_object_mut()
        && let Some(Value::Object(nested)) = fields.remove("platform_specific_config")
    {
        fields.extend(nested);
    }
    Some(entry)
}

fn platform_layer(platform: &LegacyPlatform) -> Layer<'_> {
    Layer {
        cookies: nonblank(platform.cookies.as_deref()),
        tokens: Some(&platform.fields),
        extras: Some(&platform.fields),
        selected: platform.selected,
    }
}

fn plan(
    platforms: &[LegacyPlatform],
    templates: &[LegacyTemplate],
    streamers: &[LegacyStreamer],
) -> Plan {
    let mut plan = Plan::default();
    // Identical material on one platform resolves to one profile.
    let mut shared: HashMap<(String, String), usize> = HashMap::new();
    let by_key: HashMap<&str, &LegacyPlatform> = platforms
        .iter()
        .map(|platform| (platform.key.as_str(), platform))
        .collect();
    let templates_by_key: HashMap<&str, &LegacyTemplate> = templates
        .iter()
        .filter(|template| !template.retiring)
        .map(|template| (template.key.as_str(), template))
        .collect();
    let mut add = |platform: &LegacyPlatform,
                   name: &str,
                   chain: &[Layer<'_>],
                   scope: &'static str|
     -> Option<usize> {
        let login =
            crate::credentials::platform_reauth_extra(&platform.name, Some(&platform.fields));
        let material = derive_material(&platform.name, chain, login)?;
        if let Err(error) = material.validate(&platform.name) {
            warn!(
                scope,
                name,
                platform = %platform.name,
                %error,
                "Configured credentials are not valid account material; they are not converted"
            );
            return None;
        }
        let key = (
            platform.key.clone(),
            serde_json::json!([
                material.cookies,
                material.refresh_token,
                material.access_token,
                material.reauth_config,
            ])
            .to_string(),
        );
        if let Some(index) = shared.get(&key) {
            return Some(*index);
        }
        plan.profiles.push(NewProfile {
            platform_key: platform.key.clone(),
            label: label(name),
            material,
        });
        let index = plan.profiles.len() - 1;
        shared.insert(key, index);
        Some(index)
    };

    for platform in platforms.iter().filter(|platform| !platform.selected) {
        let layer = platform_layer(platform);
        let has_login =
            crate::credentials::platform_reauth_extra(&platform.name, Some(&platform.fields))
                .is_some();
        if (layer.owns_material() || has_login)
            && let Some(index) = add(platform, &platform.name, &[layer], "platform")
        {
            plan.platforms.insert(platform.key.clone(), index);
        }
    }

    let mut in_use: HashMap<&str, HashSet<&str>> = HashMap::new();
    for streamer in streamers.iter().filter(|streamer| !streamer.deleted) {
        if let Some(template) = &streamer.template_key {
            in_use
                .entry(template.as_str())
                .or_default()
                .insert(streamer.platform_key.as_str());
        }
    }
    for template in templates.iter().filter(|template| !template.retiring) {
        let platforms_in_use = in_use.get(template.key.as_str());
        if nonblank(template.cookies.as_deref()).is_some() && platforms_in_use.is_none() {
            warn!(
                template = %template.name,
                "Template cookies are not used by any streamer; they are not converted"
            );
        }
        let mut platforms_in_use: Vec<&&str> = platforms_in_use.into_iter().flatten().collect();
        platforms_in_use.sort();
        for platform_key in platforms_in_use {
            let Some(platform) = by_key.get(*platform_key) else {
                continue;
            };
            let entry = template.overrides.get(&platform.name);
            if entry
                .and_then(|entry| entry.get("credential_selection"))
                .is_some()
            {
                continue;
            }
            let extras = template_extras(entry);
            let layer = Layer {
                cookies: nonblank(template.cookies.as_deref()),
                tokens: entry,
                extras: extras.as_ref(),
                selected: false,
            };
            if layer.owns_material()
                && let Some(index) = add(
                    platform,
                    &template.name,
                    &[layer, platform_layer(platform)],
                    "template",
                )
            {
                plan.templates
                    .insert((template.key.clone(), platform.name.clone()), index);
            }
        }
    }

    for streamer in streamers.iter().filter(|streamer| !streamer.deleted) {
        if streamer.config.get("credential_selection").is_some() {
            continue;
        }
        let Some(platform) = by_key.get(streamer.platform_key.as_str()) else {
            continue;
        };
        let layer = Layer {
            cookies: string(Some(&streamer.config), "cookies"),
            tokens: Some(&streamer.config),
            extras: streamer.config.get("platform_extras"),
            selected: false,
        };
        if !layer.owns_material() {
            continue;
        }
        let template = streamer
            .template_key
            .as_deref()
            .and_then(|key| templates_by_key.get(key));
        let entry = template.and_then(|template| template.overrides.get(&platform.name));
        let extras = template_extras(entry);
        let mut chain = vec![layer];
        if let Some(template) = template {
            chain.push(Layer {
                cookies: nonblank(template.cookies.as_deref()),
                tokens: entry,
                extras: extras.as_ref(),
                selected: entry
                    .and_then(|entry| entry.get("credential_selection"))
                    .is_some(),
            });
        }
        chain.push(platform_layer(platform));
        if let Some(index) = add(platform, &streamer.name, &chain, "streamer") {
            plan.streamers.insert(streamer.key.clone(), index);
        }
    }

    for platform in platforms
        .iter()
        .filter(|platform| crate::domain::is_streamlink_platform(&platform.name))
    {
        localize_streamlink(&mut plan, platform, &templates_by_key, streamers);
    }
    plan
}

/// Give each Streamlink streamer that has no account of its own, in its
/// configuration or kept from storage, the converted account it inherited,
/// then drop the platform and template choices, which configuration does not
/// accept on Streamlink. A profile no streamer uses is
/// still created, so the account stays available to choose.
fn localize_streamlink(
    plan: &mut Plan,
    platform: &LegacyPlatform,
    templates: &HashMap<&str, &LegacyTemplate>,
    streamers: &[LegacyStreamer],
) {
    let platform_index = plan.platforms.remove(&platform.key);
    for streamer in streamers.iter().filter(|streamer| {
        !streamer.deleted
            && streamer.platform_key == platform.key
            && streamer.config.get("credential_selection").is_none()
            && !streamer.keeps_stored_selection
    }) {
        if plan.streamers.contains_key(&streamer.key) {
            continue;
        }
        let template = streamer
            .template_key
            .as_deref()
            .and_then(|key| templates.get(key));
        // A template that already selects decides for its streamers; a bundle
        // carrying one is converted by `localize_streamlink_selections`.
        if template
            .and_then(|template| template.overrides.get(&platform.name))
            .and_then(|entry| entry.get("credential_selection"))
            .is_some()
        {
            continue;
        }
        let inherited = template
            .and_then(|template| {
                plan.templates
                    .get(&(template.key.clone(), platform.name.clone()))
                    .copied()
            })
            .or(platform_index);
        if let Some(index) = inherited {
            plan.streamers.insert(streamer.key.clone(), index);
        }
    }
    plan.templates.retain(|(_, name), _| name != &platform.name);
}

/// A bundle can carry a platform or template selection for the Streamlink
/// platform, or a Streamlink streamer's pool, none of which configuration
/// accepts. Each Streamlink streamer without its own selection, in the bundle
/// or kept from storage (`kept`), takes the one it inherited, a pool becomes
/// its first account, and the platform and template selections are removed.
/// `none` needs no copy: a streamer without a selection is anonymous there.
fn localize_streamlink_selections(export: &mut ConfigExport, kept: &HashSet<String>) -> Result<()> {
    let mut platform_selection = None;
    for platform in &mut export.platforms {
        if crate::domain::is_streamlink_platform(&platform.platform_name) {
            platform_selection = platform.credential_selection.take();
        }
    }
    let mut template_selections: HashMap<String, CredentialSelection> = HashMap::new();
    for template in &mut export.templates {
        let Some(entries) = template
            .platform_overrides
            .as_mut()
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (name, entry) in entries.iter_mut() {
            if !crate::domain::is_streamlink_platform(name) {
                continue;
            }
            if let Some(selection) = entry
                .as_object_mut()
                .and_then(|fields| fields.remove("credential_selection"))
                .filter(|selection| !selection.is_null())
            {
                template_selections.insert(
                    template.name.clone(),
                    CredentialSelection::from_value(selection)?,
                );
            }
        }
    }
    let stored = |selection: Option<CredentialSelection>| {
        selection.filter(|selection| !matches!(selection, CredentialSelection::Inherit))
    };
    let platform_selection = stored(platform_selection);
    for streamer in &mut export.streamers {
        if !crate::domain::is_streamlink_platform(&streamer.platform) {
            continue;
        }
        let own = streamer
            .streamer_specific_config
            .as_ref()
            .and_then(|config| config.get("credential_selection"))
            .filter(|selection| !selection.is_null())
            .cloned()
            .map(CredentialSelection::from_value)
            .transpose()?;
        let selection = match stored(own) {
            Some(CredentialSelection::Pool { credential_ids, .. }) => {
                warn!(
                    streamer = %streamer.name,
                    "Streamlink streamers use one account; the imported pool keeps its first account"
                );
                credential_ids.first().map(|id| fixed(id))
            }
            Some(_) => continue,
            None if kept.contains(&streamer.url) => continue,
            None => {
                let inherited = streamer
                    .template
                    .as_ref()
                    .and_then(|template| template_selections.get(template))
                    .filter(|selection| !matches!(selection, CredentialSelection::Inherit))
                    .or(platform_selection.as_ref());
                match inherited {
                    Some(CredentialSelection::Fixed { credential_id }) => {
                        Some(fixed(credential_id))
                    }
                    Some(CredentialSelection::Pool { credential_ids, .. }) => {
                        credential_ids.first().map(|id| fixed(id))
                    }
                    _ => None,
                }
            }
        };
        let Some(selection) = selection else {
            continue;
        };
        let config = streamer
            .streamer_specific_config
            .get_or_insert_with(|| serde_json::json!({}));
        if !config.is_object() {
            *config = serde_json::json!({});
        }
        if let Some(fields) = config.as_object_mut() {
            fields.insert(
                "credential_selection".into(),
                serde_json::to_value(selection)?,
            );
        }
    }
    Ok(())
}

async fn convert_database(connection: &mut SqliteConnection) -> Result<(usize, usize)> {
    // The conversion runs right after the migration that created selections,
    // so no platform has one yet.
    let platforms: Vec<LegacyPlatform> =
        sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
            "SELECT p.id, p.platform_name, s.cookies, p.platform_specific_config FROM platform_config p LEFT JOIN legacy_cookies s ON s.scope = 'platform' AND s.id = p.id ORDER BY p.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|(key, name, cookies, fields)| LegacyPlatform {
            key,
            name,
            cookies,
            fields: object(fields),
            selected: false,
        })
        .collect();
    let templates: Vec<LegacyTemplate> =
        sqlx::query_as::<_, (String, String, Option<String>, Option<String>, bool)>(
            "SELECT t.id, t.name, s.cookies, t.platform_overrides, EXISTS(SELECT 1 FROM retirement_config_deletions WHERE kind = 'template' AND config_id = t.id) FROM template_config t LEFT JOIN legacy_cookies s ON s.scope = 'template' AND s.id = t.id ORDER BY t.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|(key, name, cookies, overrides, retiring)| LegacyTemplate {
            key,
            name,
            cookies,
            overrides: object(overrides),
            retiring,
        })
        .collect();
    let streamers: Vec<LegacyStreamer> = sqlx::query_as::<
        _,
        (String, String, String, Option<String>, Option<String>, bool),
    >(
        "SELECT id, name, platform_config_id, template_config_id, streamer_specific_config, deleted_at IS NOT NULL FROM streamers ORDER BY id",
    )
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(
        |(key, name, platform_key, template_key, config, deleted)| LegacyStreamer {
            key,
            name,
            platform_key,
            template_key,
            config: object(config),
            deleted,
            // No streamer selects accounts before this conversion.
            keeps_stored_selection: false,
        },
    )
    .collect();

    let plan = plan(&platforms, &templates, &streamers);
    let mut ids = Vec::with_capacity(plan.profiles.len());
    for profile in &plan.profiles {
        ids.push(
            credential_profiles::create_in(
                connection,
                &profile.platform_key,
                &profile.label,
                true,
                &profile.material,
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await?
            .id,
        );
    }

    // Material that converted now lives in profiles; anything left in
    // configuration would be ignored, so every scope is cleared.
    for platform in &platforms {
        let mut fields = platform.fields.clone();
        clear_authentication(&platform.name, &mut fields);
        sqlx::query("UPDATE platform_config SET platform_specific_config = ? WHERE id = ?")
            .bind(fields.to_string())
            .bind(&platform.key)
            .execute(&mut *connection)
            .await?;
        if let Some(index) = plan.platforms.get(&platform.key) {
            credential_selections::set(
                connection,
                &CredentialOwner::Platform {
                    platform_id: platform.key.clone(),
                },
                &platform.key,
                &fixed(&ids[*index]),
            )
            .await?;
        }
    }
    // Converted template and streamer selections go through the same document
    // split as configuration writes; retiring owners select nothing.
    for template in &templates {
        let mut overrides = template.overrides.clone();
        strip_overrides(&mut overrides, &template.key, &plan, &ids)?;
        let mut overrides = Some(overrides.to_string());
        let selections = credential_selections::take_overrides(&mut overrides)?;
        sqlx::query("UPDATE template_config SET platform_overrides = ? WHERE id = ?")
            .bind(overrides)
            .bind(&template.key)
            .execute(&mut *connection)
            .await?;
        if template.retiring {
            // Any template update cancels its deferred deletion; a template
            // awaiting deletion stays pending.
            sqlx::query(
                "INSERT OR IGNORE INTO retirement_config_deletions(kind, config_id) VALUES ('template', ?)",
            )
            .bind(&template.key)
            .execute(&mut *connection)
            .await?;
        } else {
            credential_selections::write_template(connection, &template.key, selections).await?;
        }
    }
    let platform_names: HashMap<&str, &str> = platforms
        .iter()
        .map(|platform| (platform.key.as_str(), platform.name.as_str()))
        .collect();
    for streamer in &streamers {
        let mut config = streamer.config.clone();
        strip_streamer(
            &mut config,
            platform_names
                .get(streamer.platform_key.as_str())
                .copied()
                .unwrap_or_default(),
            plan.streamers.get(&streamer.key).map(|index| &ids[*index]),
        )?;
        let mut config = Some(config.to_string());
        let selection = credential_selections::take_document(&mut config)?;
        sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = ?")
            .bind(config)
            .bind(&streamer.key)
            .execute(&mut *connection)
            .await?;
        if let Some(selection) = selection.filter(|_| !streamer.deleted) {
            credential_selections::set(
                connection,
                &CredentialOwner::Streamer {
                    streamer_id: streamer.key.clone(),
                },
                &streamer.platform_key,
                &selection,
            )
            .await?;
        }
    }

    let scopes = plan.platforms.len() + plan.templates.len() + plan.streamers.len();
    Ok((plan.profiles.len(), scopes))
}

fn strip_overrides(
    overrides: &mut Value,
    template_key: &str,
    plan: &Plan,
    ids: &[String],
) -> Result<()> {
    let Some(entries) = overrides.as_object_mut() else {
        return Ok(());
    };
    for key in crate::credentials::AUTHENTICATION_FIELDS {
        entries.remove(*key);
    }
    for (platform_name, entry) in entries.iter_mut() {
        clear_authentication(platform_name, entry);
    }
    for ((key, platform_name), index) in &plan.templates {
        if key != template_key {
            continue;
        }
        let entry = entries
            .entry(platform_name.clone())
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        if let Some(entry) = entry.as_object_mut() {
            entry.insert(
                "credential_selection".into(),
                serde_json::to_value(fixed(&ids[*index]))?,
            );
        }
    }
    Ok(())
}

fn strip_streamer(config: &mut Value, platform: &str, selected: Option<&String>) -> Result<()> {
    clear_authentication(platform, config);
    if let Some(id) = selected
        && let Some(fields) = config.as_object_mut()
    {
        fields.insert(
            "credential_selection".into(),
            serde_json::to_value(fixed(id))?,
        );
    }
    Ok(())
}

fn fixed(id: &str) -> CredentialSelection {
    CredentialSelection::Fixed {
        credential_id: id.to_owned(),
    }
}

fn label(name: &str) -> String {
    let limit = 128 - LABEL_SUFFIX.chars().count();
    let name = name.trim();
    let name = if name.is_empty() { "Account" } else { name };
    let truncated: String = name.chars().take(limit).collect();
    format!("{}{LABEL_SUFFIX}", truncated.trim_end())
}

/// `chain` runs from the converting scope down to the platform layer. The
/// earlier walk stopped at a scope that already had a selection, so layers
/// from there down contribute nothing.
fn derive_material(
    platform: &str,
    chain: &[Layer<'_>],
    login: Option<Value>,
) -> Option<CredentialMaterial> {
    let reachable: Vec<Layer<'_>> = chain
        .iter()
        .enumerate()
        .take_while(|(index, layer)| *index == 0 || !layer.selected)
        .map(|(_, layer)| *layer)
        .collect();
    let source = reachable.iter().find(|layer| layer.cookies.is_some());
    let mut material = CredentialMaterial {
        cookies: source
            .and_then(|layer| layer.cookies)
            .unwrap_or_default()
            .to_owned(),
        refresh_token: source
            .and_then(|layer| string(layer.tokens, "refresh_token"))
            .map(str::to_owned),
        access_token: source
            .and_then(|layer| string(layer.tokens, "access_token"))
            .map(str::to_owned),
        // Platform login applied only when the walk reached the platform.
        reauth_config: login.filter(|_| reachable.len() == chain.len()),
    };
    let mut extras = None;
    for layer in reachable.iter().rev() {
        extras = merge_platform_extras(extras, layer.extras.cloned());
    }
    copy_account_extras(&mut material, platform, &extras.unwrap_or_default());
    let has_token = material.access_token.is_some() || material.refresh_token.is_some();
    if material.cookies.trim().is_empty() && material.reauth_config.is_none() && !has_token {
        return None;
    }
    Some(material)
}

/// Account extras are part of the account: Twitch's OAuth token, and the
/// Douyin/Douyu device cookies when the platform's mode would have sent them.
fn copy_account_extras(material: &mut CredentialMaterial, platform: &str, extras: &Value) {
    if platform.eq_ignore_ascii_case("twitch") {
        // Twitch's extractor never read a generic access_token.
        material.access_token = extras
            .get("oauth_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        return;
    }
    let token = if platform.eq_ignore_ascii_case("douyin")
        && extras
            .get("ttwid_management_mode")
            .and_then(Value::as_str)
            .unwrap_or("global")
            == "per_extractor"
        && !platforms_parser::extractor::utils::parse_cookie_header(&material.cookies)
            .iter()
            .any(|(name, _)| name == "ttwid")
    {
        extras
            .get("ttwid")
            .and_then(Value::as_str)
            .map(|value| ("ttwid", value))
    } else if platform.eq_ignore_ascii_case("douyu")
        && extras
            .get("api_mode")
            .and_then(Value::as_str)
            .unwrap_or("app")
            .eq_ignore_ascii_case("app")
        && !platforms_parser::extractor::utils::extras_get_bool(Some(extras), "only_audio")
            .or_else(|| {
                platforms_parser::extractor::utils::extras_get_bool(Some(extras), "onlyAudio")
            })
            .unwrap_or(false)
    {
        extras
            .get("device_id")
            .and_then(Value::as_str)
            .map(|value| ("acf_did", value))
    } else {
        None
    };
    if let Some((name, value)) = token.filter(|(_, value)| !value.is_empty()) {
        let selected = format!("{name}={value}");
        material.cookies = merge_cookie_updates(&selected, [material.cookies.as_str()]);
    }
}

/// Remove authentication inputs while keeping content settings. Bigo's
/// `password` is a room password, so it moves to `stream_password`.
fn clear_authentication(platform: &str, fields: &mut Value) {
    if platform.eq_ignore_ascii_case("bigo") {
        keep_room_password(fields);
    }
    crate::credentials::remove_authentication_fields(platform, fields);
}

/// Moves a Bigo `password` to `stream_password` unless one is set, at every
/// level of configuration.
fn keep_room_password(fields: &mut Value) {
    let Some(fields) = fields.as_object_mut() else {
        return;
    };
    if fields
        .get("stream_password")
        .and_then(Value::as_str)
        .is_none()
        && let Some(password) = fields
            .get("password")
            .and_then(Value::as_str)
            .map(str::to_owned)
    {
        fields.insert("stream_password".into(), password.into());
    }
    fields.remove("password");
    for key in crate::credentials::NESTED_EXTRAS {
        if let Some(nested) = fields.get_mut(key) {
            keep_room_password(nested);
        }
    }
}
