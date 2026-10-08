//! Resolve account policy without acquiring material or consuming rotation.

use crate::Result;
use crate::database::repositories::StoredSelection;

use super::{CredentialOwner, CredentialSelection, ResolvedCredentialPolicy};

/// Configuration keys that carried account material before profiles. They are
/// never read from configuration: profiles supply them to the extractor.
/// `username`/`password` are account login only on SOOP; elsewhere they are
/// room passwords and stay.
pub const AUTHENTICATION_FIELDS: &[&str] = &[
    "reauth_config",
    "cookies",
    "refresh_token",
    "access_token",
    "oauth_token",
    "ttwid",
    "device_id",
    "session_cookies",
    "last_cookie_check_date",
    "last_cookie_check_result",
];

/// The first stored selection from streamer to platform wins; a scope that
/// stores none inherits, and with no selection at any layer the scope has no
/// managed policy. `layers` are the selections of one streamer or scope on
/// `platform_id`, in any order. On the Streamlink platform only a streamer's
/// own selection and the account of its site count, so without either it is
/// anonymous whatever a platform or template row says.
pub(crate) fn resolve_authentication(
    platform_id: &str,
    layers: &[StoredSelection],
) -> Result<Option<ResolvedCredentialPolicy>> {
    layers
        .iter()
        .filter(|layer| {
            layer.platform_id == platform_id
                && !matches!(layer.selection, CredentialSelection::Inherit)
                && (matches!(layer.owner, CredentialOwner::Streamer { .. })
                    || layer.site.is_some()
                    || !crate::domain::is_streamlink_platform(&layer.platform_name))
        })
        .min_by_key(|layer| layer.owner.precedence())
        .map(|layer| {
            ResolvedCredentialPolicy::new(
                platform_id.to_owned(),
                layer.owner.clone(),
                layer.selection.clone(),
            )
        })
        .transpose()
}

/// Objects inside configuration extras that carry extractor options too.
pub(crate) const NESTED_EXTRAS: [&str; 2] = ["platform_specific_config", "platform_extras"];

/// The keys that are account material on `platform`, `also` included.
fn account_keys<'a>(platform: &str, also: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    AUTHENTICATION_FIELDS
        .iter()
        .chain(super::login_fields(platform))
        .chain(also)
        .copied()
}

fn remove_keys(platform: &str, value: &mut serde_json::Value, also: &[&str]) {
    if let Some(fields) = value.as_object_mut() {
        for key in account_keys(platform, also) {
            fields.remove(key);
        }
        for key in NESTED_EXTRAS {
            if let Some(nested) = fields.get_mut(key) {
                remove_keys(platform, nested, also);
            }
        }
    }
}

/// Removes the account fields of `platform` from configuration `value`,
/// nested option objects included. Room passwords stay.
pub(crate) fn remove_authentication_fields(platform: &str, value: &mut serde_json::Value) {
    remove_keys(platform, value, &[]);
}

/// Whether configuration `value` sets an account field of `platform`,
/// nested option objects included.
pub(crate) fn carries_authentication_fields(platform: &str, value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|fields| {
        account_keys(platform, &[]).any(|key| fields.get(key).is_some_and(|value| !value.is_null()))
            || NESTED_EXTRAS.iter().any(|key| {
                fields
                    .get(*key)
                    .is_some_and(|nested| carries_authentication_fields(platform, nested))
            })
    })
}

/// Account fields must not survive managed, none, or explicit raw-cookie overrides.
/// Room passwords are content configuration and deliberately remain available.
fn isolate_authentication_extras(
    platform: &str,
    mut extras: serde_json::Value,
) -> serde_json::Value {
    remove_keys(
        platform,
        &mut extras,
        &[crate::database::repositories::credential_selections::SELECTION_KEY],
    );
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
        if let Some(fields) = extras.as_object_mut() {
            if let Some(password) = room_password {
                fields.insert("stream_password".into(), password.into());
            }
            fields.remove("password");
        }
    }
    isolate_authentication_extras(platform, extras)
}

/// Only the acquired account may provide extractor authentication extras.
pub fn managed_authentication_extras(
    platform: &str,
    extras: Option<serde_json::Value>,
    material: &super::CredentialMaterial,
) -> Option<serde_json::Value> {
    let mut extras = extras.map(|extras| isolate_platform_authentication_extras(platform, extras));
    let authentication = super::provider(platform).extractor_authentication(material);
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
        let configured = serde_json::json!({"oauth_token":"account-b","ttwid":"device-b","device_id":"device-b","stream_password":"room","platform_specific_config":{"oauth_token":"nested-b"},"platform_extras":{"access_token":"nested-token"}});
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
                serde_json::json!({"username":"viewer","password":"account"})
            ),
            serde_json::json!({})
        );
        // Elsewhere a password protects a room, not an account.
        assert_eq!(
            isolate_platform_authentication_extras(
                "TwitCasting",
                serde_json::json!({"password":"room"})
            ),
            serde_json::json!({"password":"room"})
        );
    }
}
