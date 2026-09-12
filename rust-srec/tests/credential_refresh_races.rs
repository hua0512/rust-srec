use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_srec::credentials::{
    CredentialError, CredentialManager, CredentialRefreshService, CredentialScope,
    CredentialSource, CredentialStatus, CredentialStore, RefreshState, RefreshedCredentials,
};
use rust_srec::database::repositories::SqlxCredentialStore;
use serde_json::json;
use tokio::sync::{Notify, Semaphore};

#[derive(Clone, Copy)]
enum Owner {
    Platform,
    Template,
    Streamer,
}

struct Fixture {
    pool: sqlx::SqlitePool,
    store: Arc<SqlxCredentialStore>,
    source: CredentialSource,
    owner: Owner,
}

impl Fixture {
    async fn new(owner: Owner) -> Self {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let scope = match owner {
            Owner::Platform => CredentialScope::Platform {
                platform_id: "platform-bilibili".into(),
                platform_name: "bilibili".into(),
            },
            Owner::Template => {
                sqlx::query("INSERT INTO template_config(id,name) VALUES ('race-template','Race')")
                    .execute(&pool)
                    .await
                    .unwrap();
                CredentialScope::Template {
                    template_id: "race-template".into(),
                    template_name: "Race".into(),
                }
            }
            Owner::Streamer => {
                use rust_srec::database::repositories::{
                    SqlxStreamerRepository, StreamerRepository,
                };
                let mut row = rust_srec::database::models::StreamerDbModel::new(
                    "Race",
                    "https://example.test/race",
                    "platform-bilibili",
                );
                row.id = "race-streamer".into();
                SqlxStreamerRepository::new(pool.clone(), pool.clone())
                    .create_streamer(&row)
                    .await
                    .unwrap();
                CredentialScope::Streamer {
                    streamer_id: "race-streamer".into(),
                    streamer_name: "Race".into(),
                }
            }
        };
        let fixture = Self {
            store: Arc::new(SqlxCredentialStore::new(pool.clone(), pool.clone())),
            pool,
            owner,
            source: CredentialSource::new(
                scope,
                "old-cookie".into(),
                Some("old-refresh".into()),
                "bilibili".into(),
            )
            .with_access_token(Some("old-access".into())),
        };
        fixture
            .save("old-cookie", "old-refresh", "old-access")
            .await;
        fixture
    }

    async fn save(&self, cookies: &str, refresh: &str, access: &str) {
        let mut connection = self.pool.acquire().await.unwrap();
        self.save_on(&mut connection, cookies, refresh, access)
            .await;
    }

    async fn save_on(
        &self,
        connection: &mut sqlx::SqliteConnection,
        cookies: &str,
        refresh: &str,
        access: &str,
    ) {
        let fields = json!({"cookies": cookies, "refresh_token": refresh, "access_token": access, "keep": 42});
        let (sql, document) = match self.owner {
            Owner::Platform => (
                "UPDATE platform_config SET cookies=?, platform_specific_config=? WHERE id=?",
                fields,
            ),
            Owner::Template => (
                "UPDATE template_config SET cookies=?, platform_overrides=? WHERE id=?",
                json!({"bilibili":fields,"unrelated":{"keep":true}}),
            ),
            Owner::Streamer => (
                "UPDATE streamers SET name=?, streamer_specific_config=? WHERE id=?",
                fields,
            ),
        };
        sqlx::query(sql)
            .bind(cookies)
            .bind(document.to_string())
            .bind(self.source.scope.record_id())
            .execute(connection)
            .await
            .unwrap();
    }

    async fn snapshot(&self) -> (Option<String>, Option<String>) {
        let mut connection = self.pool.acquire().await.unwrap();
        self.snapshot_on(&mut connection).await
    }

    async fn snapshot_on(
        &self,
        connection: &mut sqlx::SqliteConnection,
    ) -> (Option<String>, Option<String>) {
        let sql = match self.owner {
            Owner::Platform => {
                "SELECT cookies,platform_specific_config FROM platform_config WHERE id=?"
            }
            Owner::Template => "SELECT cookies,platform_overrides FROM template_config WHERE id=?",
            Owner::Streamer => "SELECT name,streamer_specific_config FROM streamers WHERE id=?",
        };
        sqlx::query_as(sql)
            .bind(self.source.scope.record_id())
            .fetch_one(connection)
            .await
            .unwrap()
    }
}

struct PausedProvider {
    pause_check: bool,
    fail_refresh: bool,
    started: Notify,
    release: Semaphore,
}

impl PausedProvider {
    async fn pause(&self) {
        self.started.notify_one();
        self.release.acquire().await.unwrap().forget();
    }
}

#[async_trait]
impl CredentialManager for PausedProvider {
    fn platform_id(&self) -> &'static str {
        "bilibili"
    }

    async fn check_status(&self, cookies: &str) -> Result<CredentialStatus, CredentialError> {
        if cookies == "manual-cookie" {
            return Ok(CredentialStatus::Valid);
        }
        if self.pause_check {
            self.pause().await;
            Ok(CredentialStatus::Invalid {
                reason: "obsolete check".into(),
                error_code: None,
            })
        } else {
            Ok(CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            })
        }
    }

    async fn refresh(&self, _: &RefreshState) -> Result<RefreshedCredentials, CredentialError> {
        self.pause().await;
        if self.fail_refresh {
            return Err(CredentialError::InvalidRefreshToken);
        }
        Ok(RefreshedCredentials {
            cookies: "obsolete-cookie".into(),
            refresh_token: Some("obsolete-refresh".into()),
            access_token: Some("obsolete-access".into()),
            expires_at: None,
        })
    }

    async fn validate(&self, _: &str) -> Result<bool, CredentialError> {
        Ok(true)
    }
}

async fn assert_new_login_survives(owner: Owner, pause_check: bool, fail_refresh: bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let fixture = Fixture::new(owner).await;
        let provider = Arc::new(PausedProvider {
            pause_check,
            fail_refresh,
            started: Notify::new(),
            release: Semaphore::new(0),
        });
        let mut service = CredentialRefreshService::new(fixture.store.clone());
        service.register_manager(provider.clone());
        let pending = service.check_and_refresh_source(&fixture.source);
        tokio::pin!(pending);
        tokio::select! {
            _ = provider.started.notified() => {},
            result = &mut pending => panic!("provider should pause: {result:?}"),
        }
        fixture
            .save("manual-cookie", "manual-refresh", "manual-access")
            .await;
        service.invalidate(&fixture.source.scope);
        let saved = fixture.snapshot().await;
        provider.release.add_permits(1);
        let result = pending.await;
        assert_eq!(
            fixture.snapshot().await,
            saved,
            "old provider result overwrote the new login"
        );
        assert!(
            matches!(result, Err(CredentialError::SourceChanged)),
            "obsolete provider result must be rejected"
        );
        let current = fixture.store.reload_source(&fixture.source).await.unwrap();
        assert!(
            service.daily_tracker().needs_check(&current),
            "obsolete status repopulated the invalidated cache"
        );
        assert_eq!(
            service
                .failure_tracker()
                .failure_count(&fixture.source.scope),
            0
        );
        assert_eq!(
            service
                .check_and_refresh_source(&fixture.source)
                .await
                .unwrap()
                .as_deref(),
            Some("manual-cookie")
        );
        assert!(!service.daily_tracker().needs_check(&current));
    })
    .await
    .expect("credential race must settle");
}

#[tokio::test]
async fn platform_refresh_cannot_overwrite_new_login() {
    assert_new_login_survives(Owner::Platform, false, false).await;
}

#[tokio::test]
async fn template_refresh_cannot_overwrite_new_login() {
    assert_new_login_survives(Owner::Template, false, false).await;
}

#[tokio::test]
async fn streamer_refresh_cannot_overwrite_new_login() {
    assert_new_login_survives(Owner::Streamer, false, false).await;
}

#[tokio::test]
async fn delayed_invalid_check_cannot_poison_new_login() {
    for owner in [Owner::Platform, Owner::Template, Owner::Streamer] {
        assert_new_login_survives(owner, true, false).await;
    }
}

#[tokio::test]
async fn delayed_refresh_failure_cannot_poison_new_login() {
    for owner in [Owner::Platform, Owner::Template, Owner::Streamer] {
        assert_new_login_survives(owner, false, true).await;
    }
}

fn replacement() -> RefreshedCredentials {
    RefreshedCredentials {
        cookies: "rotated-cookie".into(),
        refresh_token: Some("rotated-refresh".into()),
        access_token: None,
        expires_at: None,
    }
}

#[tokio::test]
async fn extracted_cookies_require_the_original_credential_source() {
    for owner in [Owner::Platform, Owner::Template, Owner::Streamer] {
        let fixture = Fixture::new(owner).await;
        let service = CredentialRefreshService::new(fixture.store.clone());
        fixture
            .save("manual-cookie", "manual-refresh", "manual-access")
            .await;
        let saved = fixture.snapshot().await;
        assert!(matches!(
            service
                .persist_session_cookies(&fixture.source, "obsolete-cookie".into())
                .await,
            Err(CredentialError::SourceChanged)
        ));
        assert_eq!(fixture.snapshot().await, saved);
        let current = fixture.store.reload_source(&fixture.source).await.unwrap();
        service
            .persist_session_cookies(&current, "session-cookie".into())
            .await
            .unwrap();
        let persisted = fixture.store.reload_source(&current).await.unwrap();
        assert_eq!(persisted.cookies, "session-cookie");
        assert_eq!(persisted.refresh_token.as_deref(), Some("manual-refresh"));
        assert_eq!(persisted.access_token.as_deref(), Some("manual-access"));
        assert!(!service.daily_tracker().needs_check(&persisted));
    }
}

#[tokio::test]
async fn credential_fields_are_compared_after_writer_admission() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for owner in [Owner::Platform, Owner::Template, Owner::Streamer] {
            let fixture = Fixture::new(owner).await;
            for (cookies, refresh, access) in [
                ("manual-cookie", "old-refresh", "old-access"),
                ("old-cookie", "manual-refresh", "old-access"),
                ("old-cookie", "old-refresh", "manual-access"),
                ("", "", ""),
            ] {
                fixture
                    .save("old-cookie", "old-refresh", "old-access")
                    .await;
                let mut edit = rust_srec::database::begin_immediate(&fixture.pool)
                    .await
                    .unwrap();
                fixture.save_on(&mut edit, cookies, refresh, access).await;
                let saved = fixture.snapshot_on(&mut edit).await;
                let replacement = replacement();
                let update = fixture
                    .store
                    .update_credentials(&fixture.source, &replacement);
                tokio::pin!(update);
                assert!(futures::poll!(update.as_mut()).is_pending());
                edit.commit().await.unwrap();
                assert!(matches!(update.await, Err(CredentialError::SourceChanged)));
                assert_eq!(fixture.snapshot().await, saved);
            }
        }
    })
    .await
    .expect("writer contention must settle");
}

#[tokio::test]
async fn cookie_refresh_is_allowed_after_unrelated_configuration_edits() {
    for owner in [Owner::Platform, Owner::Template, Owner::Streamer] {
        let fixture = Fixture::new(owner).await;
        let sql = match owner {
            Owner::Platform => {
                "UPDATE platform_config SET platform_specific_config=json_set(platform_specific_config,'$.keep',99) WHERE id=?"
            }
            Owner::Template => {
                "UPDATE template_config SET platform_overrides=json_set(platform_overrides,'$.unrelated.keep',99) WHERE id=?"
            }
            Owner::Streamer => {
                "UPDATE streamers SET streamer_specific_config=json_set(streamer_specific_config,'$.keep',99) WHERE id=?"
            }
        };
        sqlx::query(sql)
            .bind(fixture.source.scope.record_id())
            .execute(&fixture.pool)
            .await
            .unwrap();
        fixture
            .store
            .update_credentials(&fixture.source, &replacement())
            .await
            .unwrap();
        let current = fixture.store.reload_source(&fixture.source).await.unwrap();
        assert_eq!(current.cookies, "rotated-cookie");
        assert_eq!(current.access_token.as_deref(), Some("old-access"));
        let raw: serde_json::Value =
            serde_json::from_str(&fixture.snapshot().await.1.unwrap()).unwrap();
        assert_eq!(
            if matches!(owner, Owner::Template) {
                &raw["unrelated"]["keep"]
            } else {
                &raw["keep"]
            },
            99
        );
    }
}

#[tokio::test]
async fn changed_password_rejects_refresh_including_inherited_reauth() {
    for owner in [Owner::Platform, Owner::Template, Owner::Streamer] {
        let mut fixture = Fixture::new(owner).await;
        fixture.source.platform_name = "soop".into();
        if matches!(owner, Owner::Platform) {
            fixture.source.scope = CredentialScope::Platform {
                platform_id: "platform-soop".into(),
                platform_name: "soop".into(),
            };
        }
        fixture
            .save("old-cookie", "old-refresh", "old-access")
            .await;
        sqlx::query("UPDATE platform_config SET platform_specific_config=json_set(COALESCE(platform_specific_config,'{}'),'$.username','user','$.password','old-password') WHERE id='platform-soop'")
            .execute(&fixture.pool).await.unwrap();
        let source = fixture.store.reload_source(&fixture.source).await.unwrap();
        assert!(source.has_reauth_extra());
        sqlx::query("UPDATE platform_config SET platform_specific_config=json_set(platform_specific_config,'$.password','new-password') WHERE id='platform-soop'")
            .execute(&fixture.pool).await.unwrap();
        let saved = fixture.snapshot().await;
        assert!(matches!(
            fixture
                .store
                .update_credentials(&source, &replacement())
                .await,
            Err(CredentialError::SourceChanged)
        ));
        assert_eq!(fixture.snapshot().await, saved);
    }
}
