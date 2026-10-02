//! Resolve account policy without acquiring material or consuming rotation.

use crate::Result;
use crate::database::models::{PlatformConfigDbModel, TemplateConfigDbModel};
use crate::domain::streamer::Streamer;

use super::{
    CredentialOwner, CredentialScope, CredentialSelection, CredentialSource,
    ResolvedCredentialPolicy,
};

struct Layer {
    owner: CredentialOwner,
    scope: CredentialScope,
    selection: Option<CredentialSelection>,
    cookies: Option<String>,
    refresh_token: Option<String>,
}

pub(crate) struct ResolvedAuthentication {
    pub cookies: Option<String>,
    pub source: Option<CredentialSource>,
    pub policy: Option<ResolvedCredentialPolicy>,
    pub isolated: bool,
}

fn selection(value: Option<&serde_json::Value>) -> Result<Option<CredentialSelection>> {
    value
        .cloned()
        .map(CredentialSelection::from_value)
        .transpose()
}

pub(crate) fn resolve_authentication(
    streamer: &Streamer,
    platform: &PlatformConfigDbModel,
    template: Option<&TemplateConfigDbModel>,
) -> Result<ResolvedAuthentication> {
    let platform_specific: serde_json::Value = platform
        .platform_specific_config
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let streamer_config = streamer.streamer_specific_config.as_ref();
    let string = |value: Option<&serde_json::Value>, key: &str| {
        value
            .and_then(|v| v.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let mut layers = vec![Layer {
        owner: CredentialOwner::Streamer {
            streamer_id: streamer.id.clone(),
        },
        scope: CredentialScope::Streamer {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
        },
        selection: selection(streamer_config.and_then(|v| v.get("credential_selection")))?,
        cookies: string(streamer_config, "cookies"),
        refresh_token: string(streamer_config, "refresh_token"),
    }];
    if let Some(template) = template {
        let overrides: serde_json::Value = template
            .platform_overrides
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        let fields = overrides.get(&platform.platform_name);
        layers.push(Layer {
            owner: CredentialOwner::Template {
                template_id: template.id.clone(),
            },
            scope: CredentialScope::Template {
                template_id: template.id.clone(),
                template_name: template.name.clone(),
            },
            selection: selection(fields.and_then(|v| v.get("credential_selection")))?,
            cookies: template.cookies.clone(),
            refresh_token: string(fields, "refresh_token"),
        });
    }
    layers.push(Layer {
        owner: CredentialOwner::Platform {
            platform_id: platform.id.clone(),
        },
        scope: CredentialScope::Platform {
            platform_id: platform.id.clone(),
            platform_name: platform.platform_name.clone(),
        },
        selection: platform
            .credential_selection
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .map(CredentialSelection::from_value)
            .transpose()?,
        cookies: platform.cookies.clone(),
        refresh_token: string(Some(&platform_specific), "refresh_token"),
    });
    let reauth = super::platform_reauth_extra(&platform.platform_name, Some(&platform_specific));
    // Account extras (Twitch OAuth, Douyin ttwid, Douyu device ID) are legacy
    // authentication material too, even when no layer configures cookies.
    let account_extras = legacy_account_extras(streamer, platform, template);
    let legacy_account = ["oauth_token", "ttwid", "device_id"].iter().any(|key| {
        account_extras
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .is_some()
    });
    resolve_layers(layers, platform, reauth, legacy_account)
}

fn resolve_layers(
    layers: Vec<Layer>,
    platform: &PlatformConfigDbModel,
    reauth: Option<serde_json::Value>,
    legacy_account: bool,
) -> Result<ResolvedAuthentication> {
    let mut resolved = ResolvedAuthentication {
        cookies: None,
        source: None,
        policy: None,
        isolated: false,
    };
    let mut legacy_scope_seen = legacy_account;
    let mut platform_legacy = false;
    for layer in layers {
        match layer.selection {
            Some(CredentialSelection::Inherit) => {
                resolved.isolated = true;
                continue;
            }
            Some(policy) => {
                resolved.isolated = true;
                if !legacy_scope_seen {
                    resolved.policy = Some(ResolvedCredentialPolicy::new(
                        platform.id.clone(),
                        layer.owner,
                        policy,
                    )?);
                }
                break;
            }
            None => {}
        }
        platform_legacy = matches!(layer.owner, CredentialOwner::Platform { .. });
        if let Some(cookies) = layer.cookies {
            legacy_scope_seen = true;
            if resolved.cookies.is_none() {
                resolved.cookies = Some(cookies.clone());
            }
            if resolved.source.is_none() && !cookies.trim().is_empty() {
                resolved.source = Some(CredentialSource::new(
                    layer.scope,
                    cookies,
                    layer.refresh_token,
                    platform.platform_name.clone(),
                ));
            }
        }
    }
    // Legacy local cookies may use platform login only through an uninterrupted
    // legacy platform layer. A managed boundary never lends another account's login.
    if platform_legacy {
        if resolved.source.is_none() && reauth.is_some() {
            resolved.source = Some(CredentialSource::new(
                CredentialScope::Platform {
                    platform_id: platform.id.clone(),
                    platform_name: platform.platform_name.clone(),
                },
                String::new(),
                None,
                platform.platform_name.clone(),
            ));
        }
        if let Some(source) = &mut resolved.source {
            source.reauth_extra = reauth;
        }
    }
    if resolved.policy.is_none()
        && !legacy_scope_seen
        && resolved.source.is_none()
        && resolved.isolated
    {
        resolved.policy = Some(ResolvedCredentialPolicy::new(
            platform.id.clone(),
            CredentialOwner::Platform {
                platform_id: platform.id.clone(),
            },
            CredentialSelection::None,
        )?);
    }
    Ok(resolved)
}

/// Account fields must not survive managed, none, or explicit raw-cookie overrides.
/// Room passwords are content configuration and deliberately remain available.
pub fn isolate_authentication_extras(mut extras: serde_json::Value) -> serde_json::Value {
    if let Some(fields) = extras.as_object_mut() {
        for key in [
            "credential_selection",
            "reauth_config",
            "cookies",
            "username",
            "password",
            "refresh_token",
            "access_token",
            "oauth_token",
            "ttwid",
            "device_id",
            "session_cookies",
            "last_cookie_check_date",
            "last_cookie_check_result",
        ] {
            fields.remove(key);
        }
        if let Some(nested) = fields.get_mut("platform_specific_config") {
            *nested = isolate_authentication_extras(nested.take());
        }
        if let Some(nested) = fields.get_mut("platform_extras") {
            *nested = isolate_authentication_extras(nested.take());
        }
    }
    extras
}

/// Normalize platform content aliases before removing configured account inputs.
pub fn isolate_platform_authentication_extras(
    platform: &str,
    mut extras: serde_json::Value,
) -> serde_json::Value {
    if platform.eq_ignore_ascii_case("bigo") {
        let room_password = extras
            .get("stream_password")
            .and_then(serde_json::Value::as_str)
            .or_else(|| extras.get("password").and_then(serde_json::Value::as_str))
            .map(str::to_owned);
        if let (Some(fields), Some(password)) = (extras.as_object_mut(), room_password) {
            fields.insert("stream_password".into(), password.into());
        }
    }
    isolate_authentication_extras(extras)
}

/// Only the acquired account may provide extractor authentication extras.
pub fn managed_authentication_extras(
    platform: &str,
    extras: Option<serde_json::Value>,
    material: &super::CredentialMaterial,
) -> Option<serde_json::Value> {
    let mut extras = extras.map(|extras| isolate_platform_authentication_extras(platform, extras));
    let mut authentication = serde_json::Map::new();
    if platform.eq_ignore_ascii_case("soop")
        && let Some(login) = material
            .reauth_config
            .as_ref()
            .and_then(serde_json::Value::as_object)
    {
        for key in ["username", "password"] {
            if let Some(value) = login.get(key).and_then(serde_json::Value::as_str) {
                authentication.insert(key.into(), value.into());
            }
        }
    } else if platform.eq_ignore_ascii_case("twitch")
        && let Some(token) = &material.access_token
    {
        authentication.insert("oauth_token".into(), token.clone().into());
    }
    if !authentication.is_empty() {
        let fields = extras.get_or_insert_with(|| serde_json::json!({}));
        if !fields.is_object() {
            *fields = serde_json::json!({});
        }
        if let Some(fields) = fields.as_object_mut() {
            fields.extend(authentication);
        }
    }
    extras
}

/// Reproduce supported account extras only within the uninterrupted legacy layers.
/// Template wrappers flatten before the existing non-null shallow overlay rule.
pub(crate) fn legacy_account_extras(
    streamer: &Streamer,
    platform: &PlatformConfigDbModel,
    template: Option<&TemplateConfigDbModel>,
) -> serde_json::Value {
    use platforms_parser::extractor::platform_configs::merge_platform_extras;
    let platform_fields = platform
        .platform_specific_config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let platform_policy = platform
        .credential_selection
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let template_fields = template
        .and_then(|template| template.platform_overrides.as_deref())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|overrides| overrides.get(&platform.platform_name).cloned());
    let template_policy = template_fields
        .as_ref()
        .and_then(|fields| fields.get("credential_selection"))
        .cloned();
    let template_extras = template_fields.map(|mut fields| {
        if let Some(fields) = fields.as_object_mut()
            && let Some(serde_json::Value::Object(nested)) =
                fields.remove("platform_specific_config")
        {
            fields.extend(nested);
        }
        fields
    });
    let streamer_fields = streamer.streamer_specific_config.as_ref();
    let layers = [
        (
            streamer_fields
                .and_then(|fields| fields.get("credential_selection"))
                .cloned(),
            streamer_fields
                .and_then(|fields| fields.get("platform_extras"))
                .cloned(),
        ),
        (template_policy, template_extras),
        (platform_policy, platform_fields),
    ];
    let mut configured = None;
    for (_, extras) in layers.iter().rev() {
        configured = merge_platform_extras(configured, extras.clone());
    }
    let mut allowed = Vec::new();
    for (policy, extras) in layers {
        if let Some(policy) = policy {
            if policy.get("mode").and_then(serde_json::Value::as_str) == Some("inherit") {
                continue;
            }
            break;
        }
        allowed.push(extras);
    }
    let mut merged = None;
    for extras in allowed.into_iter().rev() {
        merged = merge_platform_extras(merged, extras);
    }
    let mut fields = merged
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let platform_name = platform.platform_name.to_ascii_lowercase();
    fields.retain(|key, _| match platform_name.as_str() {
        "twitch" => key == "oauth_token",
        "douyin" => key == "ttwid",
        "douyu" => key == "device_id",
        _ => false,
    });
    let controls: &[&str] = match platform_name.as_str() {
        "douyin" => &["ttwid_management_mode"],
        "douyu" => &["api_mode", "only_audio", "onlyAudio", "device_id_mode"],
        _ => &[],
    };
    for key in controls {
        if let Some(value) = configured.as_ref().and_then(|fields| fields.get(*key)) {
            fields.insert((*key).into(), value.clone());
        }
    }
    serde_json::Value::Object(fields)
}

/// Preserve provider-generated device/guest cookies while the selected snapshot
/// remains authoritative for every name already belonging to its account.
pub fn merge_cookie_updates<'a>(
    selected: &'a str,
    updates: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut cookies: Vec<(String, String)> = Vec::new();
    let mut positions = std::collections::HashMap::new();
    for input in updates.into_iter().chain(std::iter::once(selected)) {
        for token in input
            .split(';')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let name = token.split_once('=').map_or(token, |(name, _)| name.trim());
            if let Some(&position) = positions.get(name) {
                cookies[position] = (name.to_string(), token.to_string());
            } else {
                positions.insert(name.to_string(), cookies.len());
                cookies.push((name.to_string(), token.to_string()));
            }
        }
    }
    cookies
        .into_iter()
        .map(|(_, token)| token)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod authentication_isolation_tests {
    use super::*;

    #[test]
    fn configured_tokens_devices_and_logins_cannot_cross_a_managed_or_raw_boundary() {
        let configured = serde_json::json!({"oauth_token":"account-b","username":"b","password":"secret","ttwid":"device-b","device_id":"device-b","stream_password":"room","platform_specific_config":{"oauth_token":"nested-b"},"platform_extras":{"password":"nested-password"}});
        let isolated = isolate_platform_authentication_extras("Twitch", configured);
        assert_eq!(
            isolated,
            serde_json::json!({"stream_password":"room","platform_specific_config":{},"platform_extras":{}})
        );
        let selected = super::super::CredentialMaterial {
            cookies: String::new(),
            refresh_token: None,
            access_token: Some("account-a".into()),
            reauth_config: None,
        };
        let bound = managed_authentication_extras("Twitch", Some(isolated), &selected).unwrap();
        assert_eq!(bound["oauth_token"], "account-a");
        assert_eq!(bound["stream_password"], "room");
        assert!(!bound.to_string().contains("account-b"));
    }

    #[test]
    fn bigo_room_password_alias_is_preserved_without_leaking_login_passwords() {
        let alias = isolate_platform_authentication_extras(
            "Bigo",
            serde_json::json!({"password":"room","oauth_token":"account"}),
        );
        assert_eq!(alias, serde_json::json!({"stream_password":"room"}));
        let canonical = isolate_platform_authentication_extras(
            "bigo",
            serde_json::json!({"password":"alias","stream_password":"canonical"}),
        );
        assert_eq!(
            canonical,
            serde_json::json!({"stream_password":"canonical"})
        );
        assert_eq!(
            isolate_platform_authentication_extras(
                "Soop",
                serde_json::json!({"password":"account"})
            ),
            serde_json::json!({})
        );
    }

    #[test]
    fn legacy_extras_keep_higher_account_and_stop_at_managed_boundary() {
        let mut platform: PlatformConfigDbModel = serde_json::from_value(
            serde_json::json!({"id":"p","platform_name":"twitch","created_at":0,"updated_at":0}),
        )
        .unwrap();
        platform.platform_specific_config = Some(r#"{"oauth_token":"platform"}"#.into());
        platform.credential_selection = Some(r#"{"mode":"none"}"#.into());
        let mut streamer = Streamer::new(
            "test",
            crate::domain::StreamerUrl::from_trusted("https://twitch.tv/test"),
            "p",
        );
        streamer.streamer_specific_config = Some(
            serde_json::json!({"cookies":"sid=local","platform_extras":{"oauth_token":"local"}}),
        );
        assert_eq!(
            legacy_account_extras(&streamer, &platform, None),
            serde_json::json!({"oauth_token":"local"})
        );
        streamer.streamer_specific_config =
            Some(serde_json::json!({"platform_extras":{"oauth_token":"local"}}));
        let resolved = resolve_authentication(&streamer, &platform, None).unwrap();
        assert!(
            resolved.policy.is_none(),
            "an OAuth-only higher legacy override must not become the lower anonymous policy"
        );
        assert!(resolved.isolated);
        streamer.streamer_specific_config = Some(
            serde_json::json!({"credential_selection":{"mode":"inherit"},"platform_extras":{"oauth_token":"local"}}),
        );
        assert_eq!(
            legacy_account_extras(&streamer, &platform, None),
            serde_json::json!({})
        );
    }

    #[test]
    fn inherit_above_cookie_less_device_accounts_stays_legacy() {
        for (name, extras, key) in [
            (
                "douyu",
                r#"{"device_id":"D","api_mode":"app"}"#,
                "device_id",
            ),
            ("douyin", r#"{"ttwid":"T"}"#, "ttwid"),
        ] {
            let mut platform: PlatformConfigDbModel = serde_json::from_value(
                serde_json::json!({"id":"p","platform_name":name,"created_at":0,"updated_at":0}),
            )
            .unwrap();
            platform.platform_specific_config = Some(extras.into());
            let mut streamer = Streamer::new(
                "test",
                crate::domain::StreamerUrl::from_trusted("https://example.test/room"),
                "p",
            );
            streamer.streamer_specific_config =
                Some(serde_json::json!({"credential_selection":{"mode":"inherit"}}));
            let resolved = resolve_authentication(&streamer, &platform, None).unwrap();
            assert!(
                resolved.policy.is_none(),
                "{name}: inherit skips only its own layer; the platform {key} remains legacy"
            );
            assert!(
                legacy_account_extras(&streamer, &platform, None)
                    .get(key)
                    .is_some()
            );
        }
    }

    #[test]
    fn legacy_template_wrapper_and_null_overlay_match_extractor_merge() {
        let mut platform: PlatformConfigDbModel = serde_json::from_value(
            serde_json::json!({"id":"p","platform_name":"twitch","created_at":0,"updated_at":0}),
        )
        .unwrap();
        platform.platform_specific_config = Some(r#"{"oauth_token":"platform"}"#.into());
        let mut template = TemplateConfigDbModel::new("T");
        template.platform_overrides = Some(r#"{"twitch":{"oauth_token":"flat","platform_specific_config":{"oauth_token":"nested"}}}"#.into());
        let mut streamer = Streamer::new(
            "test",
            crate::domain::StreamerUrl::from_trusted("https://twitch.tv/test"),
            "p",
        );
        streamer.streamer_specific_config =
            Some(serde_json::json!({"platform_extras":{"oauth_token":null}}));
        assert_eq!(
            legacy_account_extras(&streamer, &platform, Some(&template))["oauth_token"],
            "nested"
        );
        template.platform_overrides = Some(
            r#"{"twitch":{"oauth_token":"flat","platform_specific_config":{"oauth_token":null}}}"#
                .into(),
        );
        assert_eq!(
            legacy_account_extras(&streamer, &platform, Some(&template))["oauth_token"],
            "platform"
        );
    }
}
