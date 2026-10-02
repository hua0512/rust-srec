//! Explicit conversion captures one legacy bundle under the configuration writer.

use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};

use crate::database::models::{PlatformConfigDbModel, StreamerDbModel, TemplateConfigDbModel};
use crate::database::repositories::credential_profiles::{self, CredentialProfileRepository};
use crate::streamer::state_store::{StateChange, StatePublication};
use crate::{Error, Result};

use super::{
    CredentialMaterial, CredentialOwner, CredentialScope, CredentialSelection, ProfileError,
};

// A preview fingerprint proves equality without exposing a reusable hash of a
// low-entropy password. Restart expires previews, not committed conversions.
static PREVIEW_KEY: LazyLock<[u8; 32]> = LazyLock::new(rand::random);

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConversionSource {
    Effective,
    RefreshSource,
    None,
}

#[derive(Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertLegacyRequest {
    pub owner: CredentialOwner,
    pub platform_id: String,
    pub label: String,
    pub source: ConversionSource,
    pub expected_fingerprint: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConversionPreview {
    pub owner: CredentialOwner,
    pub platform_id: String,
    pub effective_source: Option<CredentialOwner>,
    pub refresh_source: Option<CredentialOwner>,
    pub copies_platform_login: bool,
    pub copies_account_extras: bool,
    pub source_choice_required: bool,
    pub effective_has_material: bool,
    pub refresh_has_material: bool,
    pub fingerprint: String,
    pub existing_profile_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConversionResult {
    pub profile_id: Option<String>,
    pub selection: CredentialSelection,
}

pub struct CredentialConversionService {
    pool: SqlitePool,
    writer: Arc<crate::database::CommittedWriter>,
    profiles: Arc<CredentialProfileRepository>,
    streamers: Arc<crate::streamer::CommittedStreamerState>,
}

impl CredentialConversionService {
    pub(crate) fn new(
        pool: SqlitePool,
        writer: Arc<crate::database::CommittedWriter>,
        profiles: Arc<CredentialProfileRepository>,
        streamers: Arc<crate::streamer::CommittedStreamerState>,
    ) -> Self {
        Self {
            pool,
            writer,
            profiles,
            streamers,
        }
    }

    pub async fn preview(
        &self,
        owner: CredentialOwner,
        platform_id: String,
    ) -> Result<ConversionPreview> {
        let mut transaction = self.pool.begin().await?;
        let snapshot = load_snapshot(&mut transaction, &owner, &platform_id).await?;
        snapshot.preview(owner, platform_id)
    }

    pub async fn convert(&self, request: ConvertLegacyRequest) -> Result<ConversionResult> {
        if let CredentialOwner::Streamer { streamer_id } = &request.owner {
            let id = streamer_id.clone();
            self.streamers
                .transaction(
                    "convert streamer credentials",
                    StatePublication::Metadata,
                    move |connection| {
                        Box::pin(async move {
                            let result = convert_in(connection, &request).await?;
                            let row = sqlx::query_as::<_, StreamerDbModel>(
                                "SELECT * FROM streamers WHERE id = ?",
                            )
                            .bind(id)
                            .fetch_one(&mut *connection)
                            .await?;
                            Ok(StateChange::row(result, Some(row)))
                        })
                    },
                )
                .await
        } else {
            let owner = request.owner.clone();
            let profiles = self.profiles.clone();
            self.writer
                .transaction(
                    "convert legacy credentials",
                    move |connection| {
                        Box::pin(async move { convert_in(connection, &request).await })
                    },
                    move |_| profiles.publish_owner(owner),
                )
                .await
        }
    }
}

struct Snapshot {
    platform: PlatformConfigDbModel,
    template: Option<TemplateConfigDbModel>,
    streamer: Option<StreamerDbModel>,
    current_selection: Option<CredentialSelection>,
    effective_owner: Option<CredentialOwner>,
    refresh_owner: Option<CredentialOwner>,
    effective: CredentialMaterial,
    refresh: Option<CredentialMaterial>,
    inputs: serde_json::Value,
    copies_account_extras: bool,
}

impl Snapshot {
    fn fingerprint(&self) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(*PREVIEW_KEY);
        hash.update(serde_json::to_vec(&self.inputs)?);
        hash.update(*PREVIEW_KEY);
        Ok(hex::encode(hash.finalize()))
    }

    fn preview(&self, owner: CredentialOwner, platform_id: String) -> Result<ConversionPreview> {
        Ok(ConversionPreview {
            owner,
            platform_id,
            effective_source: self.effective_owner.clone(),
            refresh_source: self.refresh_owner.clone(),
            copies_platform_login: self.effective.reauth_config.is_some(),
            copies_account_extras: self.copies_account_extras,
            source_choice_required: self.effective_owner != self.refresh_owner
                || self
                    .refresh
                    .as_ref()
                    .is_some_and(|material| material.cookies != self.effective.cookies),
            effective_has_material: self
                .effective
                .validate(&self.platform.platform_name)
                .is_ok(),
            refresh_has_material: self
                .refresh
                .as_ref()
                .is_some_and(|material| material.validate(&self.platform.platform_name).is_ok()),
            fingerprint: self.fingerprint()?,
            existing_profile_id: match &self.current_selection {
                Some(CredentialSelection::Fixed { credential_id }) => Some(credential_id.clone()),
                _ => None,
            },
        })
    }
}

fn object(raw: Option<&str>) -> Result<serde_json::Value> {
    let value = raw
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        return Err(Error::validation(
            "legacy authentication configuration must be an object",
        ));
    }
    Ok(value)
}

fn owner_of(scope: &CredentialScope) -> CredentialOwner {
    match scope {
        CredentialScope::Platform { platform_id, .. } => CredentialOwner::Platform {
            platform_id: platform_id.clone(),
        },
        CredentialScope::Template { template_id, .. } => CredentialOwner::Template {
            template_id: template_id.clone(),
        },
        CredentialScope::Streamer { streamer_id, .. } => CredentialOwner::Streamer {
            streamer_id: streamer_id.clone(),
        },
    }
}

async fn load_snapshot(
    connection: &mut SqliteConnection,
    owner: &CredentialOwner,
    platform_id: &str,
) -> Result<Snapshot> {
    credential_profiles::require_owner(connection, owner, platform_id).await?;
    let platform =
        sqlx::query_as::<_, PlatformConfigDbModel>("SELECT * FROM platform_config WHERE id = ?")
            .bind(platform_id)
            .fetch_one(&mut *connection)
            .await?;
    let streamer = if let CredentialOwner::Streamer { streamer_id } = owner {
        Some(
            sqlx::query_as::<_, StreamerDbModel>(
                "SELECT * FROM streamers WHERE id = ? AND deleted_at IS NULL",
            )
            .bind(streamer_id)
            .fetch_one(&mut *connection)
            .await?,
        )
    } else {
        None
    };
    let template_id = match owner {
        CredentialOwner::Template { template_id } => Some(template_id.as_str()),
        _ => streamer
            .as_ref()
            .and_then(|row| row.template_config_id.as_deref()),
    };
    let template = if let Some(id) = template_id {
        Some(
            sqlx::query_as::<_, TemplateConfigDbModel>(
                "SELECT * FROM template_config WHERE id = ?",
            )
            .bind(id)
            .fetch_one(&mut *connection)
            .await?,
        )
    } else {
        None
    };
    let platform_fields = object(platform.platform_specific_config.as_deref())?;
    let template_fields = object(
        template
            .as_ref()
            .and_then(|row| row.platform_overrides.as_deref()),
    )?;
    let streamer_fields = object(
        streamer
            .as_ref()
            .and_then(|row| row.streamer_specific_config.as_deref()),
    )?;
    let current_policy = match owner {
        CredentialOwner::Platform { .. } => platform
            .credential_selection
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        CredentialOwner::Template { .. } => template_fields
            .get(&platform.platform_name)
            .and_then(|fields| fields.get("credential_selection"))
            .cloned(),
        CredentialOwner::Streamer { .. } => streamer_fields.get("credential_selection").cloned(),
    };
    let current_selection = current_policy
        .map(CredentialSelection::from_value)
        .transpose()?;
    let mut entity = crate::domain::Streamer::new(
        "conversion",
        crate::domain::StreamerUrl::from_trusted("https://example.invalid/conversion"),
        platform_id,
    );
    if let Some(row) = &streamer {
        entity.id = row.id.clone();
        entity.name = row.name.clone();
        entity.streamer_specific_config = Some(streamer_fields.clone());
    }
    if let Some(row) = &template {
        entity.template_config_id = Some(row.id.clone());
    }
    let resolved =
        super::resolution::resolve_authentication(&entity, &platform, template.as_ref())?;
    if current_selection.is_none() && resolved.policy.is_some() {
        return Err(ProfileError::InvalidMaterial(
            "authentication is already inherited from a managed policy",
        )
        .into());
    }
    let mut effective_owner = None;
    let mut effective_fields = serde_json::json!({});
    let mut layers = Vec::new();
    if let Some(row) = &streamer {
        layers.push((
            CredentialOwner::Streamer {
                streamer_id: row.id.clone(),
            },
            streamer_fields
                .get("cookies")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            streamer_fields.clone(),
        ));
    }
    if let Some(row) = &template {
        layers.push((
            CredentialOwner::Template {
                template_id: row.id.clone(),
            },
            row.cookies.clone(),
            template_fields
                .get(&platform.platform_name)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        ));
    }
    let mut platform_layer = platform_fields.clone();
    if let Some(raw) = &platform.credential_selection {
        platform_layer["credential_selection"] = serde_json::from_str(raw)?;
    }
    layers.push((
        CredentialOwner::Platform {
            platform_id: platform.id.clone(),
        },
        platform.cookies.clone(),
        platform_layer,
    ));
    for (layer_owner, cookies, fields) in &layers {
        if let Some(policy) = fields.get("credential_selection") {
            if matches!(
                CredentialSelection::from_value(policy.clone())?,
                CredentialSelection::Inherit
            ) {
                continue;
            }
            break;
        }
        if cookies.is_some() {
            effective_owner = Some(layer_owner.clone());
            effective_fields = fields.clone();
            break;
        }
    }
    let login = resolved
        .source
        .as_ref()
        .and_then(|source| source.reauth_extra.clone());
    let string = |fields: &serde_json::Value, key: &str| {
        fields
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let mut effective = CredentialMaterial {
        cookies: resolved.cookies.unwrap_or_default(),
        refresh_token: string(&effective_fields, "refresh_token"),
        access_token: string(&effective_fields, "access_token"),
        reauth_config: login.clone(),
    };
    let refresh_owner = resolved
        .source
        .as_ref()
        .map(|source| owner_of(&source.scope));
    let mut refresh = resolved.source.map(|source| {
        let fields = layers
            .iter()
            .find(|(owner, _, _)| *owner == owner_of(&source.scope))
            .map(|(_, _, fields)| fields);
        CredentialMaterial {
            cookies: source.cookies,
            refresh_token: fields.and_then(|fields| string(fields, "refresh_token")),
            access_token: fields.and_then(|fields| string(fields, "access_token")),
            reauth_config: login,
        }
    });
    let account_extras =
        super::resolution::legacy_account_extras(&entity, &platform, template.as_ref());
    let copies_account_extras =
        copy_account_extras(&mut effective, &platform.platform_name, &account_extras);
    if let Some(material) = &mut refresh {
        copy_account_extras(material, &platform.platform_name, &account_extras);
    }
    let inputs = serde_json::json!({ "owner":owner, "platform_id":platform_id, "platform_name":platform.platform_name, "platform_cookies":platform.cookies, "platform_fields":platform_fields, "platform_selection":platform.credential_selection, "template_id":template.as_ref().map(|row| &row.id), "template_cookies":template.as_ref().and_then(|row| row.cookies.as_ref()), "template_fields":template_fields, "streamer_fields":streamer_fields });
    Ok(Snapshot {
        platform,
        template,
        streamer,
        current_selection,
        effective_owner,
        refresh_owner,
        effective,
        refresh,
        inputs,
        copies_account_extras,
    })
}

fn copy_account_extras(
    material: &mut CredentialMaterial,
    platform: &str,
    extras: &serde_json::Value,
) -> bool {
    if platform.eq_ignore_ascii_case("twitch") {
        // Generic access_token was not read by Twitch's legacy extractor.
        material.access_token = extras
            .get("oauth_token")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        return material.access_token.is_some();
    }
    let token = if platform.eq_ignore_ascii_case("douyin")
        && extras
            .get("ttwid_management_mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("global")
            == "per_extractor"
        && !platforms_parser::extractor::utils::parse_cookie_header(&material.cookies)
            .iter()
            .any(|(name, _)| name == "ttwid")
    {
        extras
            .get("ttwid")
            .and_then(serde_json::Value::as_str)
            .map(|value| ("ttwid", value))
    } else if platform.eq_ignore_ascii_case("douyu")
        && extras
            .get("api_mode")
            .and_then(serde_json::Value::as_str)
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
            .and_then(serde_json::Value::as_str)
            .map(|value| ("acf_did", value))
    } else {
        None
    };
    if let Some((name, value)) = token.filter(|(_, value)| !value.is_empty()) {
        let selected = format!("{name}={value}");
        material.cookies = super::merge_cookie_updates(&selected, [material.cookies.as_str()]);
        true
    } else {
        false
    }
}

fn clear_authentication(platform: &str, fields: &mut serde_json::Value) {
    if let Some(fields) = fields.as_object_mut() {
        if platform.eq_ignore_ascii_case("bigo")
            && fields
                .get("stream_password")
                .and_then(serde_json::Value::as_str)
                .is_none()
            && let Some(password) = fields
                .get("password")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        {
            fields.insert("stream_password".into(), password.into());
        }
        for key in [
            "cookies",
            "refresh_token",
            "access_token",
            "oauth_token",
            "ttwid",
            "device_id",
            "username",
            "password",
            "session_cookies",
            "last_cookie_check_date",
            "last_cookie_check_result",
        ] {
            fields.remove(key);
        }
        if let Some(nested) = fields.get_mut("platform_specific_config") {
            clear_authentication(platform, nested);
        }
        if let Some(nested) = fields.get_mut("platform_extras") {
            clear_authentication(platform, nested);
        }
    }
}

async fn convert_in(
    connection: &mut SqliteConnection,
    request: &ConvertLegacyRequest,
) -> Result<ConversionResult> {
    let snapshot = load_snapshot(connection, &request.owner, &request.platform_id).await?;
    // A repeated successful conversion never creates a second account. A later
    // managed edit is authoritative and cannot be undone by a stale preview.
    if let Some(selection) = snapshot.current_selection {
        return match selection {
            CredentialSelection::Fixed { ref credential_id } => Ok(ConversionResult {
                profile_id: Some(credential_id.clone()),
                selection,
            }),
            CredentialSelection::None => Ok(ConversionResult {
                profile_id: None,
                selection,
            }),
            _ => Err(ProfileError::SourceChanged.into()),
        };
    }
    if snapshot.fingerprint()? != request.expected_fingerprint {
        return Err(ProfileError::SourceChanged.into());
    }
    let profile = match request.source {
        ConversionSource::None => None,
        ConversionSource::Effective => Some(
            credential_profiles::create_in(
                connection,
                &request.owner,
                &request.platform_id,
                &request.label,
                true,
                &snapshot.effective,
            )
            .await?,
        ),
        ConversionSource::RefreshSource => {
            let material = snapshot
                .refresh
                .as_ref()
                .ok_or(ProfileError::InvalidMaterial(
                    "no legacy refresh source exists",
                ))?;
            Some(
                credential_profiles::create_in(
                    connection,
                    &request.owner,
                    &request.platform_id,
                    &request.label,
                    true,
                    material,
                )
                .await?,
            )
        }
    };
    let selection = match &profile {
        Some(profile) => CredentialSelection::Fixed {
            credential_id: profile.id.clone(),
        },
        None => CredentialSelection::None,
    };
    let value = serde_json::to_value(&selection)?;
    let now = crate::database::time::now_ms();
    match &request.owner {
        CredentialOwner::Platform { platform_id } => {
            let mut fields = object(snapshot.platform.platform_specific_config.as_deref())?;
            clear_authentication(&snapshot.platform.platform_name, &mut fields);
            sqlx::query("UPDATE platform_config SET credential_selection = ?, cookies = NULL, platform_specific_config = ? WHERE id = ?")
                .bind(value.to_string()).bind(fields.to_string()).bind(platform_id).execute(&mut *connection).await?;
        }
        CredentialOwner::Template { template_id } => {
            let mut fields = object(
                snapshot
                    .template
                    .as_ref()
                    .and_then(|row| row.platform_overrides.as_deref()),
            )?;
            let entry = fields
                .as_object_mut()
                .ok_or(ProfileError::InvalidMaterial(
                    "template overrides must be an object",
                ))?
                .entry(snapshot.platform.platform_name.clone())
                .or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                return Err(ProfileError::InvalidMaterial(
                    "template platform override must be an object",
                )
                .into());
            }
            clear_authentication(&snapshot.platform.platform_name, entry);
            entry["credential_selection"] = value;
            // Top-level template cookies/check markers may still belong to
            // other platforms. Only this canonical platform entry is cleared.
            sqlx::query(
                "UPDATE template_config SET platform_overrides = ?, updated_at = ? WHERE id = ?",
            )
            .bind(fields.to_string())
            .bind(now)
            .bind(template_id)
            .execute(&mut *connection)
            .await?;
        }
        CredentialOwner::Streamer { streamer_id } => {
            let mut fields = object(
                snapshot
                    .streamer
                    .as_ref()
                    .and_then(|row| row.streamer_specific_config.as_deref()),
            )?;
            clear_authentication(&snapshot.platform.platform_name, &mut fields);
            fields["credential_selection"] = value;
            sqlx::query("UPDATE streamers SET streamer_specific_config = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL").bind(fields.to_string()).bind(now).bind(streamer_id).execute(&mut *connection).await?;
        }
    }
    credential_profiles::validate_graph(connection).await?;
    Ok(ConversionResult {
        profile_id: profile.map(|profile| profile.id),
        selection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn fixture() -> (SqlitePool, Arc<CredentialConversionService>) {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let supervisor =
            Arc::new(crate::utils::task_supervisor::TaskSupervisor::for_committed_work());
        let writer = Arc::new(
            crate::database::CommittedWriter::new(pool.clone(), supervisor.clone()).unwrap(),
        );
        let profiles = Arc::new(
            CredentialProfileRepository::new(pool.clone(), pool.clone())
                .with_supervisor(supervisor),
        );
        let streamers = Arc::new(crate::streamer::CommittedStreamerState::new(
            writer.clone(),
            crate::config::ConfigEventBroadcaster::new(),
        ));
        let service = Arc::new(CredentialConversionService::new(
            pool.clone(),
            writer,
            profiles,
            streamers,
        ));
        (pool, service)
    }

    #[tokio::test]
    async fn blank_local_cookie_requires_explicit_conversion_source_and_replays_idempotently() {
        let (pool, service) = fixture().await;
        sqlx::query("UPDATE platform_config SET cookies = 'parent=secret', platform_specific_config = '{\"refresh_token\":\"refresh-secret\"}' WHERE id = 'platform-bilibili'").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO streamers(id,name,url,platform_config_id,state,priority,streamer_specific_config) VALUES ('local','Local','https://live.bilibili.com/1','platform-bilibili','NOT_LIVE','NORMAL','{\"cookies\":\"\",\"keep\":42}')").execute(&pool).await.unwrap();
        let owner = CredentialOwner::Streamer {
            streamer_id: "local".into(),
        };
        let preview = service
            .preview(owner.clone(), "platform-bilibili".into())
            .await
            .unwrap();
        assert!(preview.source_choice_required);
        assert!(!preview.effective_has_material);
        assert!(preview.refresh_has_material);
        let public = serde_json::to_string(&preview).unwrap();
        assert!(!public.contains("parent=secret"));
        assert!(!public.contains("refresh-secret"));
        let request = ConvertLegacyRequest {
            owner: owner.clone(),
            platform_id: "platform-bilibili".into(),
            label: "Parent account".into(),
            source: ConversionSource::Effective,
            expected_fingerprint: preview.fingerprint,
        };
        assert!(service.convert(request.clone()).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "invalid material rolls back profile creation");
        let request = ConvertLegacyRequest {
            source: ConversionSource::RefreshSource,
            ..request
        };
        let result = service.convert(request.clone()).await.unwrap();
        assert_eq!(
            service.convert(request).await.unwrap().profile_id,
            result.profile_id
        );
        let profile = service
            .profiles
            .get(result.profile_id.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(profile.cookies, "parent=secret");
        assert_eq!(profile.refresh_token.as_deref(), Some("refresh-secret"));
        let current = service
            .streamers
            .cache
            .metadata
            .get("local")
            .unwrap()
            .clone();
        let fields: serde_json::Value =
            serde_json::from_str(current.streamer_specific_config.as_deref().unwrap()).unwrap();
        assert!(fields.get("cookies").is_none());
        assert_eq!(fields["keep"], 42);
        assert_eq!(fields["credential_selection"]["mode"], "fixed");
    }

    #[tokio::test]
    async fn template_soop_conversion_copies_login_and_preserves_other_platform_material() {
        let (pool, service) = fixture().await;
        sqlx::query("UPDATE platform_config SET platform_specific_config = '{\"username\":\"viewer\",\"password\":\"before-secret\",\"stream_password\":\"room\"}' WHERE id = 'platform-soop'").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO template_config(id,name,cookies,platform_overrides) VALUES ('shared','Shared','session=template','{\"soop\":{\"refresh_token\":\"old\",\"quality\":\"best\"},\"bilibili\":{\"refresh_token\":\"other\"},\"last_cookie_check_result\":\"valid\"}')").execute(&pool).await.unwrap();
        let owner = CredentialOwner::Template {
            template_id: "shared".into(),
        };
        let preview = service
            .preview(owner.clone(), "platform-soop".into())
            .await
            .unwrap();
        assert!(preview.copies_platform_login);
        let result = service
            .convert(ConvertLegacyRequest {
                owner,
                platform_id: "platform-soop".into(),
                label: "SOOP account".into(),
                source: ConversionSource::Effective,
                expected_fingerprint: preview.fingerprint,
            })
            .await
            .unwrap();
        let id = result.profile_id.unwrap();
        let (cookies, raw): (String, String) = sqlx::query_as(
            "SELECT cookies,platform_overrides FROM template_config WHERE id='shared'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cookies, "session=template");
        let fields: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(fields["bilibili"]["refresh_token"], "other");
        assert_eq!(fields["last_cookie_check_result"], "valid");
        assert_eq!(fields["soop"]["quality"], "best");
        assert!(fields["soop"].get("refresh_token").is_none());
        sqlx::query("UPDATE platform_config SET platform_specific_config = '{\"username\":\"viewer\",\"password\":\"after-secret\"}' WHERE id='platform-soop'").execute(&pool).await.unwrap();
        let profile = service.profiles.get(&id).await.unwrap();
        assert_eq!(
            profile.material().unwrap().reauth_config.unwrap()["password"],
            "before-secret"
        );
    }

    #[tokio::test]
    async fn converting_one_template_platform_rejects_only_its_stale_legacy_writer() {
        use crate::credentials::{
            CredentialError, CredentialSource, CredentialStore, RefreshedCredentials,
        };
        let (pool, service) = fixture().await;
        sqlx::query("INSERT INTO template_config(id,name,cookies,platform_overrides) VALUES ('shared','Shared','session=old','{\"bilibili\":{},\"huya\":{}}')").execute(&pool).await.unwrap();
        let owner = CredentialOwner::Template {
            template_id: "shared".into(),
        };
        let scope = CredentialScope::Template {
            template_id: "shared".into(),
            template_name: "Shared".into(),
        };
        let old_bilibili =
            CredentialSource::new(scope.clone(), "session=old".into(), None, "bilibili".into());
        let old_huya = CredentialSource::new(scope, "session=old".into(), None, "huya".into());
        let preview = service
            .preview(owner.clone(), "platform-bilibili".into())
            .await
            .unwrap();
        let result = service
            .convert(ConvertLegacyRequest {
                owner,
                platform_id: "platform-bilibili".into(),
                label: "Bilibili".into(),
                source: ConversionSource::Effective,
                expected_fingerprint: preview.fingerprint,
            })
            .await
            .unwrap();
        let replacement = RefreshedCredentials {
            cookies: "session=rotated".into(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        };
        let store =
            crate::database::repositories::SqlxCredentialStore::new(pool.clone(), pool.clone());
        assert!(matches!(
            store.update_credentials(&old_bilibili, &replacement).await,
            Err(CredentialError::SourceChanged)
        ));
        store
            .update_credentials(&old_huya, &replacement)
            .await
            .unwrap();
        let copied = service
            .profiles
            .get(result.profile_id.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(copied.cookies, "session=old");
        let remaining: String =
            sqlx::query_scalar("SELECT cookies FROM template_config WHERE id='shared'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, "session=rotated");
    }

    #[tokio::test]
    async fn twitch_oauth_only_conversion_preserves_exact_effective_token_and_clears_extras() {
        let (pool, service) = fixture().await;
        sqlx::query("UPDATE platform_config SET cookies=NULL,platform_specific_config='{\"oauth_token\":\"platform-token\",\"access_token\":\"unused-token\",\"quality\":\"keep\"}' WHERE id='platform-twitch'").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO template_config(id,name,platform_overrides) VALUES ('oauth-template','OAuth template','{\"twitch\":{\"oauth_token\":\"flat-token\",\"platform_specific_config\":{\"oauth_token\":\"nested-token\"}}}')").execute(&pool).await.unwrap();
        for (owner, token) in [
            (
                CredentialOwner::Platform {
                    platform_id: "platform-twitch".into(),
                },
                "platform-token",
            ),
            (
                CredentialOwner::Template {
                    template_id: "oauth-template".into(),
                },
                "nested-token",
            ),
        ] {
            let preview = service
                .preview(owner.clone(), "platform-twitch".into())
                .await
                .unwrap();
            assert!(preview.effective_has_material);
            assert!(preview.copies_account_extras);
            assert!(!serde_json::to_string(&preview).unwrap().contains(token));
            let result = service
                .convert(ConvertLegacyRequest {
                    owner,
                    platform_id: "platform-twitch".into(),
                    label: "Twitch account".into(),
                    source: ConversionSource::Effective,
                    expected_fingerprint: preview.fingerprint,
                })
                .await
                .unwrap();
            let profile = service
                .profiles
                .get(result.profile_id.as_ref().unwrap())
                .await
                .unwrap();
            assert!(profile.cookies.is_empty());
            assert_eq!(profile.access_token.as_deref(), Some(token));
        }
        let fields: String = sqlx::query_scalar(
            "SELECT platform_specific_config FROM platform_config WHERE id='platform-twitch'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fields).unwrap(),
            serde_json::json!({"quality":"keep"})
        );
        let template: String = sqlx::query_scalar(
            "SELECT platform_overrides FROM template_config WHERE id='oauth-template'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!template.contains("nested-token"));
        assert!(!template.contains("flat-token"));
    }

    #[test]
    fn conversion_copies_only_device_inputs_used_by_the_legacy_adapter() {
        let cases = [
            (
                "douyin",
                "sid=account",
                serde_json::json!({"ttwid":"device","ttwid_management_mode":"per_extractor"}),
                Some(("ttwid", "device")),
            ),
            (
                "douyin",
                "sid=account; ttwid=own",
                serde_json::json!({"ttwid":"other","ttwid_management_mode":"per_extractor"}),
                Some(("ttwid", "own")),
            ),
            (
                "douyin",
                "sid=account",
                serde_json::json!({"ttwid":"ignored","ttwid_management_mode":"global"}),
                None,
            ),
            (
                "douyin",
                "sid=account",
                serde_json::json!({"ttwid":"ignored","ttwid_management_mode":"unknown"}),
                None,
            ),
            (
                "douyu",
                "sid=account; acf_did=old",
                serde_json::json!({"device_id":"new","api_mode":"app"}),
                Some(("acf_did", "new")),
            ),
            (
                "douyu",
                "sid=account; acf_did=old",
                serde_json::json!({"device_id":"ignored","api_mode":"web"}),
                Some(("acf_did", "old")),
            ),
            (
                "douyu",
                "sid=account; acf_did=old",
                serde_json::json!({"device_id":"ignored","only_audio":true}),
                Some(("acf_did", "old")),
            ),
            (
                "douyu",
                "sid=account; acf_did=old",
                serde_json::json!({"device_id":"new","only_audio":false,"onlyAudio":true}),
                Some(("acf_did", "new")),
            ),
        ];
        for (platform, cookies, extras, expected) in cases {
            let mut material = CredentialMaterial {
                cookies: cookies.into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            };
            copy_account_extras(&mut material, platform, &extras);
            let cookies =
                platforms_parser::extractor::utils::parse_cookie_header(&material.cookies);
            assert!(
                cookies
                    .iter()
                    .any(|(name, value)| name == "sid" && value == "account")
            );
            if let Some((name, value)) = expected {
                assert!(cookies.iter().any(
                    |(actual_name, actual_value)| actual_name == name && actual_value == value
                ));
            } else {
                assert!(!cookies.iter().any(|(name, _)| name == "ttwid"));
            }
        }
        let mut fields = serde_json::json!({"password":"room", "stream_password":null, "oauth_token":"account", "platform_extras":{"password":"nested-room", "stream_password":5}});
        clear_authentication("bigo", &mut fields);
        assert_eq!(
            fields,
            serde_json::json!({"stream_password":"room", "platform_extras":{"stream_password":"nested-room"}})
        );
    }

    #[tokio::test]
    async fn stale_preview_is_rejected_before_any_material_or_policy_changes() {
        let (pool, service) = fixture().await;
        sqlx::query("UPDATE platform_config SET cookies='before' WHERE id='platform-bilibili'")
            .execute(&pool)
            .await
            .unwrap();
        let owner = CredentialOwner::Platform {
            platform_id: "platform-bilibili".into(),
        };
        let preview = service
            .preview(owner.clone(), "platform-bilibili".into())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE platform_config SET cookies='manual-login' WHERE id='platform-bilibili'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let error = service
            .convert(ConvertLegacyRequest {
                owner,
                platform_id: "platform-bilibili".into(),
                label: "Account".into(),
                source: ConversionSource::Effective,
                expected_fingerprint: preview.fingerprint,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::CredentialProfile(ProfileError::SourceChanged)
        ));
        let (cookies, policy): (String, Option<String>) = sqlx::query_as(
            "SELECT cookies,credential_selection FROM platform_config WHERE id='platform-bilibili'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cookies, "manual-login");
        assert!(policy.is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM credential_profiles")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn conversion_publication_finishes_after_committed_caller_is_aborted() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (pool, service) = fixture().await;
            sqlx::query("UPDATE platform_config SET cookies='before' WHERE id='platform-bilibili'")
                .execute(&pool)
                .await
                .unwrap();
            let owner = CredentialOwner::Platform {
                platform_id: "platform-bilibili".into(),
            };
            let preview = service
                .preview(owner.clone(), "platform-bilibili".into())
                .await
                .unwrap();
            let published = Arc::new(tokio::sync::Notify::new());
            let signal = published.clone();
            service
                .profiles
                .bind_publication(Arc::new(move |_| signal.notify_one()));
            let gate = Arc::new(crate::database::committed_writer::CommitTestGate::default());
            service.writer.set_commit_gate(
                crate::database::committed_writer::CommitPhase::AfterCommit,
                Some(gate.clone()),
            );
            let task_service = service.clone();
            let mut task = tokio::spawn(async move {
                task_service
                    .convert(ConvertLegacyRequest {
                        owner,
                        platform_id: "platform-bilibili".into(),
                        label: "Account".into(),
                        source: ConversionSource::Effective,
                        expected_fingerprint: preview.fingerprint,
                    })
                    .await
            });
            tokio::select! {
                () = gate.started.notified() => {}
                result = &mut task => panic!("conversion ended before reaching commit gate: {result:?}"),
            }
            task.abort();
            gate.release.notify_one();
            published.notified().await;
            let policy: String = sqlx::query_scalar(
                "SELECT credential_selection FROM platform_config WHERE id='platform-bilibili'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(policy.contains("fixed"));
        })
        .await
        .expect("committed conversion must finish publication");
    }
}
