//! Extractor-level credential authentication extras.
//!
//! Pure field-isolation and selection-resolution helpers live in
//! [`crate::domain::credential`].

use super::CredentialMaterial;

/// Only the acquired account may provide extractor authentication extras.
pub fn managed_authentication_extras(
    platform: &str,
    extras: Option<serde_json::Value>,
    material: &CredentialMaterial,
) -> Option<serde_json::Value> {
    let mut extras = extras.map(|extras| {
        crate::domain::credential::isolate_platform_authentication_extras(platform, extras)
    });
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

#[cfg(test)]
mod authentication_isolation_tests {
    use super::*;
    use crate::domain::credential::isolate_platform_authentication_extras;

    #[test]
    fn configured_tokens_devices_and_logins_cannot_cross_a_managed_or_raw_boundary() {
        let configured = serde_json::json!({"oauth_token":"account-b","ttwid":"device-b","device_id":"device-b","stream_password":"room","platform_specific_config":{"oauth_token":"nested-b"},"platform_extras":{"access_token":"nested-token"}});
        let isolated = isolate_platform_authentication_extras("Twitch", configured);
        assert_eq!(
            isolated,
            serde_json::json!({"stream_password":"room","platform_specific_config":{},"platform_extras":{}})
        );
        let selected = CredentialMaterial {
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
        assert_eq!(
            isolate_platform_authentication_extras(
                "TwitCasting",
                serde_json::json!({"password":"room"})
            ),
            serde_json::json!({"password":"room"})
        );
    }
}
