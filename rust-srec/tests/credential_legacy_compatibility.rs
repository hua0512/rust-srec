//! Consumer-visible legacy inputs: merged cookies deliberately differ from refresh provenance.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_srec::config::ConfigResolver;
use rust_srec::credentials::{
    CredentialError, CredentialManager, CredentialRefreshService, CredentialScope,
    CredentialStatus, RefreshState, RefreshedCredentials,
};
use rust_srec::database::repositories::{SqlxConfigRepository, SqlxCredentialStore};
use rust_srec::domain::{Streamer, StreamerUrl};
use serde_json::{Value, json};
use sqlx::SqlitePool;

struct Fixture {
    pool: SqlitePool,
    resolver: ConfigResolver<SqlxConfigRepository>,
    streamer: Streamer,
    platform_name: String,
}

impl Fixture {
    async fn new(platform: &str) -> Self {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let platform_id = format!("platform-{platform}");
        let platform_name =
            sqlx::query_scalar("SELECT platform_name FROM platform_config WHERE id = ?")
                .bind(&platform_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO template_config(id, name) VALUES ('legacy-template', 'Legacy')")
            .execute(&pool)
            .await
            .unwrap();
        let mut streamer = Streamer::new(
            "Legacy",
            StreamerUrl::from_trusted("https://example.test/legacy"),
            &platform_id,
        )
        .with_template("legacy-template");
        streamer.id = "legacy-streamer".into();
        sqlx::query("INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state) VALUES (?, ?, ?, ?, ?, 'NOT_LIVE')")
            .bind(&streamer.id).bind(&streamer.name).bind(streamer.url.as_str()).bind(&platform_id).bind("legacy-template").execute(&pool).await.unwrap();
        Self {
            resolver: ConfigResolver::new(Arc::new(SqlxConfigRepository::new(
                pool.clone(),
                pool.clone(),
            ))),
            pool,
            streamer,
            platform_name,
        }
    }

    async fn set(
        &mut self,
        platform: Option<&str>,
        template: Option<&str>,
        streamer: Option<Value>,
    ) {
        sqlx::query(
            "UPDATE platform_config SET cookies = ?, platform_specific_config = ? WHERE id = ?",
        )
        .bind(platform)
        .bind(json!({"refresh_token":"platform-token", "quality":80}).to_string())
        .bind(&self.streamer.platform_config_id)
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE template_config SET cookies = ?, platform_overrides = ? WHERE id = 'legacy-template'")
            .bind(template).bind(json!({&self.platform_name:{"refresh_token":"template-token", "quality":120}}).to_string()).execute(&self.pool).await.unwrap();
        self.streamer.streamer_specific_config = streamer;
        sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = ?")
            .bind(
                self.streamer
                    .streamer_specific_config
                    .as_ref()
                    .map(Value::to_string),
            )
            .bind(&self.streamer.id)
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

#[derive(Clone, Copy, Debug)]
enum Cookie {
    Absent,
    Null,
    Empty,
    Whitespace,
    Value,
}

impl Cookie {
    fn scalar(self, text: &'static str) -> Option<&'static str> {
        match self {
            Self::Absent | Self::Null => None,
            Self::Empty => Some(""),
            Self::Whitespace => Some(" \t "),
            Self::Value => Some(text),
        }
    }

    fn streamer(self) -> Option<Value> {
        match self {
            Self::Absent => None,
            Self::Null => Some(json!({"cookies":null,"refresh_token":"streamer-token"})),
            _ => {
                Some(json!({"cookies": self.scalar("streamer=1"),"refresh_token":"streamer-token"}))
            }
        }
    }
}

#[tokio::test]
async fn legacy_cookie_matrix_keeps_present_recording_value_and_nonblank_refresh_source_separate() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut f = Fixture::new("bilibili").await;
        let values = [
            Cookie::Absent,
            Cookie::Null,
            Cookie::Empty,
            Cookie::Whitespace,
            Cookie::Value,
        ];
        for platform in values {
            for template in values {
                for streamer in values {
                    f.set(
                        platform.scalar("platform=1"),
                        template.scalar("template=1"),
                        streamer.streamer(),
                    )
                    .await;
                    let context = f
                        .resolver
                        .resolve_context_for_streamer(&f.streamer)
                        .await
                        .unwrap();
                    let expected_recording = streamer
                        .scalar("streamer=1")
                        .or(template.scalar("template=1"))
                        .or(platform.scalar("platform=1"));
                    assert_eq!(
                        context.config.cookies.as_deref(),
                        expected_recording,
                        "recording input {platform:?}/{template:?}/{streamer:?}"
                    );
                    // The golden precedence applies to every legacy consumer of merged
                    // config (download and danmu); refresh provenance ignores blank values.
                    let expected_source = match (streamer, template, platform) {
                        (Cookie::Value, _, _) => {
                            Some(("streamer=1", "streamer-token", "legacy-streamer"))
                        }
                        (_, Cookie::Value, _) => {
                            Some(("template=1", "template-token", "legacy-template"))
                        }
                        (_, _, Cookie::Value) => {
                            Some(("platform=1", "platform-token", "platform-bilibili"))
                        }
                        _ => None,
                    };
                    assert_eq!(
                        context.credential_source.as_ref().map(|s| (
                            s.cookies.as_str(),
                            s.refresh_token.as_deref().unwrap(),
                            s.scope.record_id()
                        )),
                        expected_source,
                        "refresh input {platform:?}/{template:?}/{streamer:?}"
                    );
                    assert!(context.config.credential_policy.is_none());
                    let serialized = serde_json::to_value(context.config.as_ref()).unwrap();
                    for secret in ["platform-token", "template-token", "streamer-token"] {
                        assert!(!serialized.to_string().contains(secret));
                    }
                }
            }
        }
    })
    .await
    .expect("legacy precedence matrix must finish");
}

#[derive(Clone, Copy)]
enum Repair {
    Noop,
    Success,
    Failure,
}

struct FakeManager {
    repair: Repair,
}

#[async_trait]
impl CredentialManager for FakeManager {
    fn platform_id(&self) -> &'static str {
        "bilibili"
    }
    async fn check_status(&self, cookies: &str) -> Result<CredentialStatus, CredentialError> {
        assert!(!cookies.trim().is_empty());
        Ok(match self.repair {
            Repair::Noop => CredentialStatus::Valid,
            _ => CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            },
        })
    }
    async fn refresh(&self, state: &RefreshState) -> Result<RefreshedCredentials, CredentialError> {
        assert!(
            state
                .refresh_token
                .as_ref()
                .is_some_and(|value| value.ends_with("-token"))
        );
        match self.repair {
            Repair::Success => Ok(RefreshedCredentials {
                cookies: "refreshed=1".into(),
                refresh_token: Some("new-token".into()),
                access_token: None,
                expires_at: None,
            }),
            Repair::Failure => Err(CredentialError::InvalidRefreshToken),
            Repair::Noop => panic!("valid credentials must not refresh"),
        }
    }
    async fn validate(&self, _: &str) -> Result<bool, CredentialError> {
        Ok(true)
    }
}

#[tokio::test]
async fn legacy_refresh_success_noop_and_failure_preserve_the_exact_selected_owner() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for owner in ["platform", "template", "streamer"] {
            for repair in [Repair::Noop, Repair::Success, Repair::Failure] {
                let mut f = Fixture::new("bilibili").await;
                f.set(Some("platform=1"), (owner != "platform").then_some("template=1"), Some(json!({"cookies": if owner == "streamer" { "streamer=1" } else { " " }, "refresh_token":"streamer-token"}))).await;
                let context = f.resolver.resolve_context_for_streamer(&f.streamer).await.unwrap();
                let source = context.credential_source.as_ref().unwrap();
                assert!(matches!((&source.scope, owner), (CredentialScope::Platform { .. }, "platform") | (CredentialScope::Template { .. }, "template") | (CredentialScope::Streamer { .. }, "streamer")));
                let mut service = CredentialRefreshService::new(Arc::new(SqlxCredentialStore::new(f.pool.clone(), f.pool.clone())));
                service.register_manager(Arc::new(FakeManager { repair }));
                let before: (Option<String>, Option<String>, Option<String>) = sqlx::query_as("SELECT p.cookies, t.cookies, json_extract(s.streamer_specific_config, '$.cookies') FROM platform_config p, template_config t, streamers s WHERE p.id = 'platform-bilibili' AND t.id = 'legacy-template' AND s.id = 'legacy-streamer'").fetch_one(&f.pool).await.unwrap();
                let result = service.check_and_refresh_source(source).await;
                let after: (Option<String>, Option<String>, Option<String>) = sqlx::query_as("SELECT p.cookies, t.cookies, json_extract(s.streamer_specific_config, '$.cookies') FROM platform_config p, template_config t, streamers s WHERE p.id = 'platform-bilibili' AND t.id = 'legacy-template' AND s.id = 'legacy-streamer'").fetch_one(&f.pool).await.unwrap();
                match repair {
                    Repair::Success => {
                        assert_eq!(result.unwrap().as_deref(), Some("refreshed=1"));
                        if owner == "platform" { assert_eq!(after.0.as_deref(), Some("refreshed=1")); } else { assert_eq!(before.0, after.0); }
                        if owner == "template" { assert_eq!(after.1.as_deref(), Some("refreshed=1")); } else { assert_eq!(before.1, after.1); }
                        if owner == "streamer" { assert_eq!(after.2.as_deref(), Some("refreshed=1")); } else { assert_eq!(before.2, after.2); }
                    }
                    Repair::Noop => { assert_eq!(result.unwrap(), None); assert_eq!(before, after); }
                    Repair::Failure => { assert!(result.is_err()); assert_eq!(before, after); }
                }
            }
        }
    }).await.expect("legacy refresh matrix must finish");
}

#[tokio::test]
async fn soop_legacy_login_inheritance_stops_at_explicit_policy_boundaries() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut f = Fixture::new("soop").await;
        f.set(None, Some("template=1"), None).await;
        sqlx::query("UPDATE platform_config SET platform_specific_config = ? WHERE id = 'platform-soop'").bind(json!({"username":"account", "password":"login-secret", "stream_password":"content-secret"}).to_string()).execute(&f.pool).await.unwrap();
        let legacy = f.resolver.resolve_context_for_streamer(&f.streamer).await.unwrap();
        assert_eq!(legacy.credential_source.as_ref().unwrap().scope.record_id(), "legacy-template");
        assert_eq!(legacy.credential_source.as_ref().unwrap().reauth_extra.as_ref().unwrap()["username"], "account");
        f.streamer.streamer_specific_config = Some(json!({"credential_selection":{"mode":"none"}}));
        let none = f.resolver.resolve_context_for_streamer(&f.streamer).await.unwrap();
        assert!(none.config.cookies.is_none());
        assert!(none.credential_source.is_none());
        let extras = none.config.platform_extras.as_ref().unwrap();
        assert!(extras.get("username").is_none());
        assert!(extras.get("password").is_none());
        assert_eq!(extras["stream_password"], "content-secret");
        f.streamer.streamer_specific_config = Some(json!({"cookies":"local=1", "credential_selection":{"mode":"inherit"}}));
        let inherited = f.resolver.resolve_context_for_streamer(&f.streamer).await.unwrap();
        assert_eq!(inherited.config.cookies.as_deref(), Some("template=1"));
        assert_eq!(inherited.credential_source.as_ref().unwrap().scope.record_id(), "legacy-template");
    }).await.expect("SOOP boundary fixture must finish");
}
