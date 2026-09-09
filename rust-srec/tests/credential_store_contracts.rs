use std::time::Duration;

use rust_srec::credentials::{
    CredentialError, CredentialScope, CredentialSource, CredentialStore, RefreshedCredentials,
};
use rust_srec::database::models::StreamerDbModel;
use rust_srec::database::repositories::{
    SqlxCredentialStore, SqlxStreamerRepository, StreamerRepository,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;

#[derive(Clone, Copy, Debug)]
enum Owner {
    Platform,
    Streamer,
}

struct Fixture {
    pool: SqlitePool,
    store: SqlxCredentialStore,
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
            Owner::Streamer => {
                let mut model = StreamerDbModel::new(
                    "credential owner",
                    "https://example.com/credentials",
                    "platform-bilibili",
                );
                model.id = "credential-owner".into();
                SqlxStreamerRepository::new(pool.clone(), pool.clone())
                    .create_streamer(&model)
                    .await
                    .unwrap();
                CredentialScope::Streamer {
                    streamer_id: model.id,
                    streamer_name: model.name,
                }
            }
        };
        Self {
            store: SqlxCredentialStore::new(pool.clone(), pool.clone()),
            pool,
            source: CredentialSource::new(
                scope,
                "old-cookie".into(),
                Some("old-refresh".into()),
                "bilibili".into(),
            ),
            owner,
        }
    }

    async fn seed(&self, raw: Option<&str>) {
        match self.owner {
            Owner::Platform => {
                sqlx::query("UPDATE platform_config SET cookies='old-cookie', platform_specific_config=? WHERE id=?")
                .bind(raw).bind(self.source.scope.record_id()).execute(&self.pool).await.unwrap();
            }
            Owner::Streamer => {
                sqlx::query("UPDATE streamers SET streamer_specific_config=?, updated_at=1234567890123 WHERE id=?")
                .bind(raw).bind(self.source.scope.record_id()).execute(&self.pool).await.unwrap();
            }
        }
    }

    async fn snapshot(&self) -> (Option<String>, Option<String>, String) {
        let sql = match self.owner {
            Owner::Platform => {
                "SELECT cookies, platform_specific_config, '' FROM platform_config WHERE id=?"
            }
            Owner::Streamer => {
                "SELECT NULL, streamer_specific_config, CAST(updated_at AS TEXT) FROM streamers WHERE id=?"
            }
        };
        sqlx::query_as(sql)
            .bind(self.source.scope.record_id())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn refreshed(refresh: bool, access: bool) -> RefreshedCredentials {
    RefreshedCredentials {
        cookies: "new-cookie=';_%\\雪".into(),
        refresh_token: refresh.then(|| "new-refresh".into()),
        access_token: access.then(|| "new-access".into()),
        expires_at: None,
    }
}

#[tokio::test]
async fn credential_token_combinations_preserve_unrelated_json_and_absent_values() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for owner in [Owner::Platform, Owner::Streamer] {
            let f = Fixture::new(owner).await;
            for refresh in [false, true] {
                for access in [false, true] {
                    for existing in [false, true] {
                        let mut original =
                            json!({"nested":{"keep":[1,"雪"]},"cookies":"old-cookie"});
                        if existing {
                            original["refresh_token"] = json!("old-refresh");
                            original["access_token"] = json!("old-access");
                        }
                        f.seed(Some(&original.to_string())).await;
                        let credentials = refreshed(refresh, access);
                        let day_before = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        f.store
                            .update_credentials(&f.source, &credentials)
                            .await
                            .unwrap();
                        let (cookies, raw, _) = f.snapshot().await;
                        let got: Value = serde_json::from_str(raw.as_deref().unwrap()).unwrap();
                        assert_eq!(got["nested"], original["nested"]);
                        for (name, supplied) in [
                            ("refresh_token", &credentials.refresh_token),
                            ("access_token", &credentials.access_token),
                        ] {
                            assert_eq!(
                                got.get(name),
                                supplied
                                    .as_ref()
                                    .map(|value| json!(value))
                                    .as_ref()
                                    .or_else(|| original.get(name))
                            );
                        }
                        match owner {
                            Owner::Platform => {
                                assert_eq!(cookies.as_deref(), Some(credentials.cookies.as_str()));
                                assert_eq!(got["cookies"], "old-cookie");
                                if refresh || access {
                                    assert_eq!(got["last_cookie_check_result"], "valid");
                                    let day_after =
                                        chrono::Utc::now().format("%Y-%m-%d").to_string();
                                    assert!(
                                        got["last_cookie_check_date"] == day_before
                                            || got["last_cookie_check_date"] == day_after
                                    );
                                } else {
                                    assert_eq!(got, original);
                                }
                            }
                            Owner::Streamer => assert_eq!(got["cookies"], credentials.cookies),
                        }
                    }
                }
            }
            f.seed(None).await;
            f.store
                .update_credentials(&f.source, &refreshed(true, true))
                .await
                .unwrap();
            let raw = f.snapshot().await.1.unwrap();
            let got: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(got["refresh_token"], "new-refresh");
            assert_eq!(got["access_token"], "new-access");
        }
    })
    .await
    .expect("credential combination matrix must finish");
}

#[tokio::test]
async fn malformed_and_non_object_json_cannot_partially_refresh_credentials() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for owner in [Owner::Platform, Owner::Streamer] {
            let f = Fixture::new(owner).await;
            for raw in ["{broken", "", "null", "[]", "7", "\"text\""] {
                f.seed(Some(raw)).await;
                let before = f.snapshot().await;
                assert!(
                    f.store
                        .update_credentials(&f.source, &refreshed(true, true))
                        .await
                        .is_err(),
                    "{owner:?}: {raw}"
                );
                assert_eq!(f.snapshot().await, before);
                if matches!(owner, Owner::Streamer) {
                    assert!(
                        f.store
                            .update_credentials(&f.source, &refreshed(false, false))
                            .await
                            .is_err()
                    );
                    assert_eq!(f.snapshot().await, before);
                }
            }
        }
        let f = Fixture::new(Owner::Platform).await;
        f.seed(Some("opaque legacy config")).await;
        f.store
            .update_credentials(&f.source, &refreshed(false, false))
            .await
            .unwrap();
        let snapshot = f.snapshot().await;
        assert_eq!(
            snapshot.0.as_deref(),
            Some(refreshed(false, false).cookies.as_str())
        );
        assert_eq!(snapshot.1.as_deref(), Some("opaque legacy config"));
    })
    .await
    .expect("invalid credential documents must finish");
}

#[tokio::test]
async fn late_token_write_failure_rolls_back_cookies_tokens_and_timestamps() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for owner in [Owner::Platform, Owner::Streamer] {
            let f = Fixture::new(owner).await;
            f.seed(Some(r#"{"cookies":"old-cookie","refresh_token":"old-refresh","access_token":"old-access","keep":42}"#)).await;
            let before = f.snapshot().await;
            let trigger = match owner {
                Owner::Platform => "CREATE TRIGGER reject_refreshed_tokens BEFORE UPDATE OF platform_specific_config ON platform_config WHEN NEW.id='platform-bilibili' BEGIN SELECT RAISE(ABORT,'injected late token failure'); END",
                Owner::Streamer => "CREATE TRIGGER reject_refreshed_tokens BEFORE UPDATE OF streamer_specific_config ON streamers WHEN NEW.id='credential-owner' AND json_extract(NEW.streamer_specific_config,'$.access_token')='new-access' BEGIN SELECT RAISE(ABORT,'injected late token failure'); END",
            };
            sqlx::query(trigger).execute(&f.pool).await.unwrap();
            assert!(f.store.update_credentials(&f.source, &refreshed(true, true)).await.is_err());
            assert_eq!(f.snapshot().await, before);
            sqlx::query("DROP TRIGGER reject_refreshed_tokens").execute(&f.pool).await.unwrap();
            f.store.update_credentials(&f.source, &refreshed(true, true)).await.unwrap();
        }
    }).await.expect("credential rollback must release its transaction");
}

#[tokio::test]
async fn missing_and_retired_owners_do_not_report_refresh_success() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let f = Fixture::new(Owner::Streamer).await;
        f.seed(Some(r#"{"cookies":"old-cookie","keep":42}"#)).await;
        sqlx::query("UPDATE streamers SET deleted_at=1234567890123 WHERE id='credential-owner'")
            .execute(&f.pool)
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert!(matches!(
            f.store
                .update_credentials(&f.source, &refreshed(true, true))
                .await,
            Err(CredentialError::NoCredentials)
        ));
        assert_eq!(f.snapshot().await, before);
        assert!(matches!(
            f.store.reload_source(&f.source).await,
            Err(CredentialError::NoCredentials)
        ));
        for scope in [
            CredentialScope::Platform {
                platform_id: "missing".into(),
                platform_name: "bilibili".into(),
            },
            CredentialScope::Template {
                template_id: "missing".into(),
                template_name: "missing".into(),
            },
            CredentialScope::Streamer {
                streamer_id: "missing".into(),
                streamer_name: "missing".into(),
            },
        ] {
            let source = CredentialSource::new(scope, "old".into(), None, "bilibili".into());
            for tokens in [false, true] {
                assert!(matches!(
                    f.store
                        .update_credentials(&source, &refreshed(tokens, tokens))
                        .await,
                    Err(CredentialError::NoCredentials)
                ));
            }
        }
    })
    .await
    .expect("missing credential owners must finish");
}

#[tokio::test]
async fn retired_template_refresh_cannot_cancel_retirement_or_change_another_owner() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let f = Fixture::new(Owner::Platform).await;
        sqlx::query("INSERT INTO template_config(id,name,cookies,platform_overrides) VALUES ('retired','Retired','old-cookie','{\"bilibili\":{\"refresh_token\":\"old-refresh\"}}'), ('other','Other','other-cookie','{\"keep\":42}')")
            .execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO retirement_config_deletions(kind,config_id) VALUES ('template','retired')")
            .execute(&f.pool).await.unwrap();
        let source = CredentialSource::new(CredentialScope::Template { template_id: "retired".into(), template_name: "Retired".into() }, "old-cookie".into(), Some("old-refresh".into()), "bilibili".into());
        let before: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as("SELECT id,cookies,platform_overrides,CAST(updated_at AS TEXT) FROM template_config ORDER BY id")
            .fetch_all(&f.pool).await.unwrap();
        for tokens in [false, true] {
            assert!(matches!(f.store.update_credentials(&source, &refreshed(tokens,tokens)).await, Err(CredentialError::NoCredentials)));
        }
        assert!(matches!(f.store.reload_source(&source).await, Err(CredentialError::NoCredentials)));
        let after: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as("SELECT id,cookies,platform_overrides,CAST(updated_at AS TEXT) FROM template_config ORDER BY id")
            .fetch_all(&f.pool).await.unwrap();
        assert_eq!(after, before);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retirement_config_deletions WHERE kind='template' AND config_id='retired'").fetch_one(&f.pool).await.unwrap(), 1);
        sqlx::query("DELETE FROM retirement_config_deletions WHERE kind='template' AND config_id='retired'").execute(&f.pool).await.unwrap();
        f.store.update_credentials(&source, &refreshed(true,true)).await.unwrap();
        let current = f.store.reload_source(&source).await.unwrap();
        assert_eq!(current.cookies, refreshed(true,true).cookies);
        assert_eq!(current.refresh_token.as_deref(), Some("new-refresh"));
        let other: (String, String) = sqlx::query_as("SELECT cookies,platform_overrides FROM template_config WHERE id='other'").fetch_one(&f.pool).await.unwrap();
        assert_eq!(other, ("other-cookie".into(), "{\"keep\":42}".into()));
    }).await.expect("retired template guard must finish");
}

#[tokio::test]
async fn check_results_preserve_json_and_reject_invalid_platform_documents() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let f = Fixture::new(Owner::Platform).await;
        f.seed(Some(
            r#"{"keep":{"number":7},"refresh_token":"keep-token"}"#,
        ))
        .await;
        f.store
            .update_check_result(&f.source.scope, "needs_refresh")
            .await
            .unwrap();
        let (cookies, raw, _) = f.snapshot().await;
        let got: Value = serde_json::from_str(raw.as_deref().unwrap()).unwrap();
        assert_eq!(cookies.as_deref(), Some("old-cookie"));
        assert_eq!(got["keep"], json!({"number":7}));
        assert_eq!(got["refresh_token"], "keep-token");
        assert_eq!(got["last_cookie_check_result"], "needs_refresh");
        for raw in ["{broken", "[]", "null"] {
            f.seed(Some(raw)).await;
            let before = f.snapshot().await;
            assert!(
                f.store
                    .update_check_result(&f.source.scope, "valid")
                    .await
                    .is_err()
            );
            assert_eq!(f.snapshot().await, before);
        }
        f.seed(None).await;
        f.store
            .update_check_result(&f.source.scope, "valid")
            .await
            .unwrap();
        let (cookies, raw, _) = f.snapshot().await;
        assert_eq!(cookies.as_deref(), Some("old-cookie"));
        let initialized: Value = serde_json::from_str(raw.as_deref().unwrap()).unwrap();
        assert_eq!(initialized["last_cookie_check_result"], "valid");
        assert!(
            chrono::NaiveDate::parse_from_str(
                initialized["last_cookie_check_date"].as_str().unwrap(),
                "%Y-%m-%d"
            )
            .is_ok()
        );
        assert!(matches!(
            f.store
                .update_check_result(
                    &CredentialScope::Platform {
                        platform_id: "missing".into(),
                        platform_name: "bilibili".into()
                    },
                    "valid"
                )
                .await,
            Err(CredentialError::NoCredentials)
        ));
        let other = Fixture::new(Owner::Streamer).await;
        other.seed(Some("opaque")).await;
        let before = other.snapshot().await;
        other
            .store
            .update_check_result(&other.source.scope, "valid")
            .await
            .unwrap();
        assert_eq!(other.snapshot().await, before);
        other
            .store
            .update_check_result(
                &CredentialScope::Template {
                    template_id: "missing".into(),
                    template_name: "missing".into(),
                },
                "valid",
            )
            .await
            .unwrap();
    })
    .await
    .expect("credential check-result contracts must finish");
}
