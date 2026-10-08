use std::time::Duration;

use sqlx::SqlitePool;

use super::*;
use crate::credentials::CredentialMaterial;
use crate::database::models::{StreamerDbModel, TemplateConfigDbModel};
use crate::database::repositories::{
    ConfigRepository, CredentialProfileRepository, SqlxConfigRepository, SqlxStreamerRepository,
    StreamerRepository,
};

struct Fixture {
    pool: SqlitePool,
    profiles: CredentialProfileRepository,
    config: SqlxConfigRepository,
    streamers: SqlxStreamerRepository,
}

async fn fixture() -> Fixture {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    Fixture {
        profiles: CredentialProfileRepository::new(pool.clone(), pool.clone()),
        config: SqlxConfigRepository::new(pool.clone(), pool.clone()),
        streamers: SqlxStreamerRepository::new(pool.clone(), pool.clone()),
        pool,
    }
}

fn material(cookies: &str) -> CredentialMaterial {
    CredentialMaterial {
        cookies: cookies.into(),
        refresh_token: None,
        access_token: None,
        reauth_config: None,
    }
}

fn fixed(id: &str) -> CredentialSelection {
    CredentialSelection::Fixed {
        credential_id: id.into(),
    }
}

fn platform(id: &str) -> CredentialOwner {
    CredentialOwner::Platform {
        platform_id: id.into(),
    }
}

impl Fixture {
    async fn profile(&self, platform_id: &str, cookies: &str) -> String {
        self.profiles
            .create(
                platform_id,
                cookies,
                true,
                &material(cookies),
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap()
            .id
    }

    async fn stored(
        &self,
        owner: &CredentialOwner,
        platform_id: &str,
    ) -> Option<CredentialSelection> {
        test_support::load_scope(&mut self.pool.acquire().await.unwrap(), owner, platform_id)
            .await
            .unwrap()
    }

    async fn rows(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM credential_selections")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn streamer(
        &self,
        url: &str,
        platform_id: &str,
        config: serde_json::Value,
    ) -> StreamerDbModel {
        let mut streamer = StreamerDbModel::new("Streamer", url, platform_id);
        streamer.streamer_specific_config = Some(config.to_string());
        self.streamers.create_streamer(&streamer).await.unwrap();
        streamer
    }
}

fn document(streamer: &StreamerDbModel) -> serde_json::Value {
    serde_json::from_str(streamer.streamer_specific_config.as_deref().unwrap()).unwrap()
}

#[tokio::test]
async fn members_must_belong_to_the_selection_platform() {
    let f = fixture().await;
    let huya = f.profile("platform-huya", "huya=1").await;
    let bilibili = f.profile("platform-bilibili", "bilibili=1").await;

    // The database refuses a member from another platform either way round.
    let selection: i64 = sqlx::query_scalar("INSERT INTO credential_selections(platform_config_id, mode) VALUES ('platform-bilibili', 'fixed') RETURNING id")
        .fetch_one(&f.pool).await.unwrap();
    for member_platform in ["platform-bilibili", "platform-huya"] {
        let error = sqlx::query("INSERT INTO credential_selection_members(selection_id, platform_config_id, position, profile_id) VALUES (?, ?, 0, ?)")
            .bind(selection).bind(member_platform).bind(&huya)
            .execute(&f.pool).await.unwrap_err();
        assert!(
            matches!(&error, sqlx::Error::Database(error) if error.is_foreign_key_violation()),
            "{error}"
        );
    }
    sqlx::query("DELETE FROM credential_selections")
        .execute(&f.pool)
        .await
        .unwrap();

    // A write names the scope whose reference cannot be used.
    let mut connection = f.pool.acquire().await.unwrap();
    for missing in [huya.as_str(), "00000000-0000-0000-0000-000000000000"] {
        let error = set(
            &mut connection,
            &platform("platform-bilibili"),
            "platform-bilibili",
            &fixed(missing),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&error, Error::CredentialProfile(ProfileError::InaccessibleReferences(references)) if references == &vec!["platform:platform-bilibili".to_owned()]),
            "{error}"
        );
    }
    assert!(
        set(
            &mut connection,
            &platform("platform-bilibili"),
            "platform-bilibili",
            &fixed(&bilibili)
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn pools_round_trip_in_saved_order_and_rewrites_are_no_ops() {
    let f = fixture().await;
    let a = f.profile("platform-bilibili", "a=1").await;
    let b = f.profile("platform-bilibili", "b=1").await;
    let pool = CredentialSelection::Pool {
        credential_ids: vec![b.clone(), a.clone()],
        strategy: PoolStrategy::RoundRobin,
        failover: false,
        max_attempts: 5,
    };
    let owner = platform("platform-bilibili");
    let mut connection = f.pool.acquire().await.unwrap();
    assert!(
        set(&mut connection, &owner, "platform-bilibili", &pool)
            .await
            .unwrap()
    );
    assert!(
        !set(&mut connection, &owner, "platform-bilibili", &pool)
            .await
            .unwrap()
    );
    assert_eq!(
        test_support::load_scope(&mut connection, &owner, "platform-bilibili")
            .await
            .unwrap(),
        Some(pool)
    );
    assert!(
        set(
            &mut connection,
            &owner,
            "platform-bilibili",
            &CredentialSelection::None
        )
        .await
        .unwrap()
    );
    assert_eq!(
        test_support::load_scope(&mut connection, &owner, "platform-bilibili")
            .await
            .unwrap(),
        Some(CredentialSelection::None)
    );
    let members: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_selection_members")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(members, 0);
}

#[tokio::test]
async fn a_selected_profile_cannot_be_deleted_and_the_refusal_names_its_selectors() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let account = f.profile("platform-bilibili", "sid=a").await;
        f.config
            .update_platform_config_with_selection(
                &f.config.get_platform_config("platform-bilibili").await.unwrap(),
                Some(&fixed(&account)),
            )
            .await
            .unwrap();
        let mut template = TemplateConfigDbModel::new("Selecting template");
        template.platform_overrides = Some(
            serde_json::json!({"bilibili": {"credential_selection": {"mode": "pool", "credential_ids": [account]}}})
                .to_string(),
        );
        f.config.create_template_config(&template).await.unwrap();
        let streamer = f
            .streamer(
                "https://live.bilibili.com/1",
                "platform-bilibili",
                serde_json::json!({"credential_selection": {"mode": "fixed", "credential_id": account}}),
            )
            .await;

        let error = f.profiles.delete(&account, 1).await.unwrap_err();
        let Error::CredentialProfile(ProfileError::Referenced(references)) = error else {
            panic!("{error}");
        };
        let named: Vec<(CredentialOwner, &str, &str)> = references
            .selections
            .iter()
            .map(|selection| {
                (
                    selection.owner.clone(),
                    selection.name.as_str(),
                    selection.platform_name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            named,
            vec![
                (platform("platform-bilibili"), "bilibili", "bilibili"),
                (
                    CredentialOwner::Template {
                        template_id: template.id.clone()
                    },
                    "Selecting template",
                    "bilibili"
                ),
                (
                    CredentialOwner::Streamer {
                        streamer_id: streamer.id.clone()
                    },
                    "Streamer",
                    "bilibili"
                ),
            ]
        );
        assert!(references.recordings.is_empty());
        assert_eq!(f.profiles.references(&account).await.unwrap(), references);
        let error = sqlx::query("DELETE FROM credential_profiles WHERE id = ?")
            .bind(&account)
            .execute(&f.pool)
            .await
            .unwrap_err();
        // SQLite reports a RESTRICT refusal with the trigger constraint code.
        assert!(error.to_string().contains("FOREIGN KEY constraint failed"), "{error:?}");
        assert!(f.profiles.get(&account).await.is_ok());
    })
    .await
    .unwrap();
}

/// One listing names every profile's selecting scopes and the live
/// recordings bound to it, reading the open sessions once for all profiles.
#[tokio::test]
async fn references_of_many_profiles_are_named_in_one_pass() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let selected = f.profile("platform-bilibili", "sid=a").await;
        let recording = f.profile("platform-bilibili", "sid=b").await;
        let unused = f.profile("platform-bilibili", "sid=c").await;
        let streamer = f
            .streamer(
                "https://live.bilibili.com/1",
                "platform-bilibili",
                serde_json::json!({"credential_selection": {"mode": "pool", "credential_ids": [selected, recording]}}),
            )
            .await;
        let owner = platform("platform-bilibili");
        let bind = |profile_id: &str| {
            serde_json::to_string(&crate::credentials::CredentialBinding {
                identity: crate::credentials::CredentialIdentity::Profile {
                    profile_id: profile_id.into(),
                },
                revision: 1,
                policy: crate::credentials::ResolvedCredentialPolicy::new(
                    owner.id().into(),
                    owner.clone(),
                    fixed(profile_id),
                )
                .unwrap(),
                epoch: 1,
            })
            .unwrap()
        };
        // A live recording with its streamer, one whose streamer row is gone,
        // and an ended one that no longer holds the account.
        for (id, streamer_id, end_time) in [
            ("live", Some(streamer.id.as_str()), None),
            ("orphan", None, None),
            ("ended", Some(streamer.id.as_str()), Some(2)),
        ] {
            sqlx::query("INSERT INTO live_sessions(id, streamer_id, streamer_name, start_time, end_time, credential_binding) VALUES (?, ?, 'Recorded name', 1, ?, ?)")
                .bind(id)
                .bind(streamer_id)
                .bind(end_time)
                .bind(bind(&recording))
                .execute(&f.pool)
                .await
                .unwrap();
        }

        let ids = vec![selected.clone(), recording.clone(), unused.clone()];
        let references = f.profiles.references_of(&ids).await.unwrap();
        assert!(!references.contains_key(&unused));
        let streamer_scope = crate::credentials::SelectionReference {
            owner: CredentialOwner::Streamer {
                streamer_id: streamer.id.clone(),
            },
            name: "Streamer".into(),
            platform_id: "platform-bilibili".into(),
            platform_name: "bilibili".into(),
        };
        assert_eq!(
            references[&selected],
            crate::credentials::ProfileReferences {
                selections: vec![streamer_scope.clone()],
                recordings: Vec::new(),
            }
        );
        assert_eq!(
            references[&recording],
            crate::credentials::ProfileReferences {
                selections: vec![streamer_scope],
                recordings: vec![
                    crate::credentials::RecordingReference {
                        session_id: "live".into(),
                        streamer_id: Some(streamer.id.clone()),
                        streamer_name: "Streamer".into(),
                    },
                    crate::credentials::RecordingReference {
                        session_id: "orphan".into(),
                        streamer_id: None,
                        streamer_name: "Recorded name".into(),
                    },
                ],
            }
        );
        // The single-profile form reads the same.
        for id in &ids {
            assert_eq!(
                f.profiles.references(id).await.unwrap(),
                references.get(id).cloned().unwrap_or_default()
            );
        }
        assert!(f.profiles.references_of(&[]).await.unwrap().is_empty());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn removed_and_retired_owners_release_their_selections() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let account = f.profile("platform-bilibili", "sid=a").await;
        let selection = serde_json::json!({"mode": "fixed", "credential_id": account});
        let mut template = TemplateConfigDbModel::new("Removed template");
        template.platform_overrides =
            Some(serde_json::json!({"bilibili": {"credential_selection": selection}}).to_string());
        f.config.create_template_config(&template).await.unwrap();
        let kept = f
            .streamer(
                "https://live.bilibili.com/1",
                "platform-bilibili",
                serde_json::json!({"credential_selection": selection}),
            )
            .await;
        let retired = f
            .streamer(
                "https://live.bilibili.com/2",
                "platform-bilibili",
                serde_json::json!({"credential_selection": selection}),
            )
            .await;
        assert_eq!(f.rows().await, 3);

        f.config.delete_template_config(&template.id).await.unwrap();
        assert_eq!(f.rows().await, 2);
        // A streamer marked deleted no longer selects, so it does not hold
        // the profile while its retirement finishes.
        assert!(
            f.streamers
                .mark_streamer_deleted(&retired.id)
                .await
                .unwrap()
        );
        assert_eq!(f.rows().await, 1);
        let owners: Vec<CredentialOwner> = f
            .profiles
            .references(&account)
            .await
            .unwrap()
            .selections
            .into_iter()
            .map(|selection| selection.owner)
            .collect();
        assert_eq!(
            owners,
            vec![CredentialOwner::Streamer {
                streamer_id: kept.id.clone()
            }]
        );
        sqlx::query("DELETE FROM streamers WHERE id = ?")
            .bind(&kept.id)
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(f.rows().await, 0);
        f.profiles.delete(&account, 1).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn omitted_selections_are_kept_and_inherit_removes_them() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let account = f.profile("platform-bilibili", "sid=a").await;
        let selection = serde_json::json!({"mode": "fixed", "credential_id": account});
        let mut streamer = f
            .streamer(
                "https://live.bilibili.com/1",
                "platform-bilibili",
                serde_json::json!({"credential_selection": selection, "record_danmu": true}),
            )
            .await;
        let owner = CredentialOwner::Streamer {
            streamer_id: streamer.id.clone(),
        };
        let stored = f.streamers.get_streamer(&streamer.id).await.unwrap();
        assert_eq!(document(&stored), serde_json::json!({"record_danmu": true}));
        assert_eq!(f.stored(&owner, "platform-bilibili").await, Some(fixed(&account)));
        for config in [None, Some(serde_json::json!({"record_danmu": false}).to_string())] {
            streamer.streamer_specific_config = config;
            f.streamers.update_streamer(&streamer).await.unwrap();
            assert_eq!(f.stored(&owner, "platform-bilibili").await, Some(fixed(&account)));
        }
        streamer.streamer_specific_config =
            Some(serde_json::json!({"credential_selection": {"mode": "inherit"}}).to_string());
        f.streamers.update_streamer(&streamer).await.unwrap();
        assert_eq!(f.stored(&owner, "platform-bilibili").await, None);
        let stored = f.streamers.get_streamer(&streamer.id).await.unwrap();
        assert_eq!(document(&stored), serde_json::json!({}));

        let mut template = TemplateConfigDbModel::new("Template");
        template.platform_overrides = Some(
            serde_json::json!({"bilibili": {"credential_selection": selection, "quality": 1}, "huya": {"credential_selection": {"mode": "none"}}})
                .to_string(),
        );
        f.config.create_template_config(&template).await.unwrap();
        let template_owner = CredentialOwner::Template {
            template_id: template.id.clone(),
        };
        template.platform_overrides =
            Some(serde_json::json!({"bilibili": {"quality": 2}}).to_string());
        f.config.update_template_config(&template).await.unwrap();
        let stored = f.config.get_template_config(&template.id).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(stored.platform_overrides.as_deref().unwrap())
                .unwrap(),
            serde_json::json!({"bilibili": {"quality": 2}})
        );
        assert_eq!(f.stored(&template_owner, "platform-bilibili").await, Some(fixed(&account)));
        assert_eq!(
            f.stored(&template_owner, "platform-huya").await,
            Some(CredentialSelection::None)
        );
        template.platform_overrides = Some(
            serde_json::json!({"huya": {"credential_selection": {"mode": "inherit"}}}).to_string(),
        );
        f.config.update_template_config(&template).await.unwrap();
        assert_eq!(f.stored(&template_owner, "platform-huya").await, None);
        assert_eq!(f.stored(&template_owner, "platform-bilibili").await, Some(fixed(&account)));

        let platform_model = f.config.get_platform_config("platform-bilibili").await.unwrap();
        let platform_owner = platform("platform-bilibili");
        f.config
            .update_platform_config_with_selection(&platform_model, Some(&CredentialSelection::None))
            .await
            .unwrap();
        f.config.update_platform_config(&platform_model).await.unwrap();
        assert_eq!(
            f.stored(&platform_owner, "platform-bilibili").await,
            Some(CredentialSelection::None)
        );
        f.config
            .update_platform_config_with_selection(&platform_model, Some(&CredentialSelection::Inherit))
            .await
            .unwrap();
        assert_eq!(f.stored(&platform_owner, "platform-bilibili").await, None);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn streamer_creation_refuses_configuration_cookies() {
    let f = fixture().await;
    let mut streamer = StreamerDbModel::new(
        "Streamer",
        "https://live.bilibili.com/1",
        "platform-bilibili",
    );
    streamer.streamer_specific_config = Some(serde_json::json!({"cookies": "sid=a"}).to_string());
    let error = f.streamers.create_streamer(&streamer).await.unwrap_err();
    assert!(matches!(error, Error::Validation(_)), "{error}");
    assert!(f.streamers.get_streamer(&streamer.id).await.is_err());
}

#[tokio::test]
async fn a_streamer_that_moves_platform_inherits_there() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let bilibili = f.profile("platform-bilibili", "sid=a").await;
        let huya = f.profile("platform-huya", "huya=a").await;
        let echoed = serde_json::json!({"credential_selection": {"mode": "fixed", "credential_id": bilibili}});
        let mut streamer = f
            .streamer("https://live.bilibili.com/1", "platform-bilibili", echoed.clone())
            .await;
        let owner = CredentialOwner::Streamer {
            streamer_id: streamer.id.clone(),
        };

        // A form that repeats the old platform's selection still saves.
        streamer.url = "https://www.huya.com/1".into();
        streamer.platform_config_id = "platform-huya".into();
        f.streamers.update_streamer(&streamer).await.unwrap();
        assert_eq!(f.stored(&owner, "platform-bilibili").await, None);
        assert_eq!(f.stored(&owner, "platform-huya").await, None);
        assert!(f.profiles.references(&bilibili).await.unwrap().is_empty());

        // A selection the request changes is checked against the new platform.
        streamer.streamer_specific_config = Some(
            serde_json::json!({"credential_selection": {"mode": "pool", "credential_ids": [bilibili]}})
                .to_string(),
        );
        let error = f.streamers.update_streamer(&streamer).await.unwrap_err();
        assert!(
            matches!(&error, Error::CredentialProfile(ProfileError::InaccessibleReferences(references)) if references == &vec![format!("streamer:{}", streamer.id)]),
            "{error}"
        );
        streamer.streamer_specific_config = Some(
            serde_json::json!({"credential_selection": {"mode": "fixed", "credential_id": huya}})
                .to_string(),
        );
        f.streamers.update_streamer(&streamer).await.unwrap();
        assert_eq!(f.stored(&owner, "platform-huya").await, Some(fixed(&huya)));

        // A raw platform change drops the selection too.
        sqlx::query("UPDATE streamers SET platform_config_id = 'platform-bilibili' WHERE id = ?")
            .bind(&streamer.id)
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(f.rows().await, 0);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cloned_templates_select_the_same_accounts() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let account = f.profile("platform-bilibili", "sid=a").await;
        let mut template = TemplateConfigDbModel::new("Source");
        template.platform_overrides = Some(
            serde_json::json!({"bilibili": {"credential_selection": {"mode": "fixed", "credential_id": account}}, "huya": {"credential_selection": {"mode": "none"}}})
                .to_string(),
        );
        f.config.create_template_config(&template).await.unwrap();
        let cloned = f.config.clone_template_config(&template.id, "Copy").await.unwrap();
        let owner = CredentialOwner::Template {
            template_id: cloned.id.clone(),
        };
        assert_eq!(f.stored(&owner, "platform-bilibili").await, Some(fixed(&account)));
        assert_eq!(f.stored(&owner, "platform-huya").await, Some(CredentialSelection::None));
        assert_eq!(f.rows().await, 4);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn template_selections_need_a_known_platform_name() {
    let f = fixture().await;
    let mut template = TemplateConfigDbModel::new("Unknown platform");
    template.platform_overrides = Some(
        serde_json::json!({"Bilibili": {"credential_selection": {"mode": "none"}}}).to_string(),
    );
    let error = f
        .config
        .create_template_config(&template)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("canonical platform name"),
        "{error}"
    );
    assert!(f.config.get_template_config(&template.id).await.is_err());
}

#[test]
fn documents_split_and_rejoin_their_selection() {
    let mut raw = Some(
        serde_json::json!({"credential_selection": {"mode": "pool", "credential_ids": ["a"]}, "quality": 1})
            .to_string(),
    );
    let selection = take_document(&mut raw).unwrap().unwrap();
    assert_eq!(
        selection,
        CredentialSelection::Pool {
            credential_ids: vec!["a".into()],
            strategy: PoolStrategy::Priority,
            failover: true,
            max_attempts: 3,
        }
    );
    assert_eq!(raw.as_deref(), Some(r#"{"quality":1}"#));
    // Text without a selection, or that is not an object, stays as written.
    for text in [r#"{ "quality" : 1 }"#, "not json"] {
        let mut raw = Some(text.to_owned());
        assert_eq!(take_document(&mut raw).unwrap(), None);
        assert_eq!(raw.as_deref(), Some(text));
    }
    let mut invalid =
        Some(r#"{"credential_selection": {"mode": "pool", "credential_ids": []}}"#.to_owned());
    assert!(take_document(&mut invalid).is_err());

    let rejoined =
        inject_document(Some(serde_json::json!({"quality": 1})), Some(&selection)).unwrap();
    assert_eq!(rejoined["credential_selection"]["strategy"], "priority");
    assert_eq!(
        inject_document(
            Some(serde_json::json!({"credential_selection": {"mode": "none"}})),
            None
        ),
        Some(serde_json::json!({}))
    );

    let mut overrides = Some(
        serde_json::json!({"bilibili": {"credential_selection": {"mode": "none"}, "quality": 1}, "huya": {"quality": 2}})
            .to_string(),
    );
    assert_eq!(
        take_overrides(&mut overrides).unwrap(),
        vec![("bilibili".to_owned(), CredentialSelection::None)]
    );
    let stored = StoredSelection {
        owner: CredentialOwner::Template {
            template_id: "t".into(),
        },
        platform_id: "platform-douyu".into(),
        platform_name: "douyu".into(),
        selection: CredentialSelection::None,
    };
    let overrides: serde_json::Value = serde_json::from_str(overrides.as_deref().unwrap()).unwrap();
    assert_eq!(
        inject_overrides(Some(overrides), &[stored]),
        Some(serde_json::json!({
            "bilibili": {"quality": 1},
            "huya": {"quality": 2},
            "douyu": {"credential_selection": {"mode": "none"}}
        }))
    );
}

#[tokio::test]
async fn streamlink_accounts_are_chosen_per_streamer_one_at_a_time() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let f = fixture().await;
        let account = f.profile("platform-streamlink", "site=a").await;
        let other = f.profile("platform-streamlink", "site=b").await;
        let streamer = f
            .streamer("https://example.test/live", "platform-streamlink", serde_json::json!({}))
            .await;
        let owner = CredentialOwner::Streamer {
            streamer_id: streamer.id.clone(),
        };
        let template = TemplateConfigDbModel::new("Streamlink");
        f.config.create_template_config(&template).await.unwrap();
        let pool = CredentialSelection::Pool {
            credential_ids: vec![account.clone(), other.clone()],
            strategy: PoolStrategy::Priority,
            failover: true,
            max_attempts: 3,
        };

        let mut connection = f.pool.acquire().await.unwrap();
        let template_owner = CredentialOwner::Template {
            template_id: template.id.clone(),
        };
        for (scope, selection) in [
            (platform("platform-streamlink"), fixed(&account)),
            (platform("platform-streamlink"), CredentialSelection::None),
            (template_owner.clone(), fixed(&account)),
            (owner.clone(), pool),
        ] {
            let error = set(&mut connection, &scope, "platform-streamlink", &selection)
                .await
                .unwrap_err();
            assert!(
                matches!(&error, Error::CredentialProfile(ProfileError::PerStreamerOnly(_))),
                "{error}"
            );
            assert_eq!(
                crate::api::error::ApiError::from(error).status,
                axum::http::StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        drop(connection);
        // Template writes through configuration take the same path.
        let mut overrides = template.clone();
        overrides.platform_overrides = Some(
            serde_json::json!({"streamlink": {"credential_selection": {"mode": "fixed", "credential_id": account}}})
                .to_string(),
        );
        let error = f.config.update_template_config(&overrides).await.unwrap_err();
        assert!(
            matches!(&error, Error::CredentialProfile(ProfileError::PerStreamerOnly(_))),
            "{error}"
        );
        // Inherit stores nothing and is always allowed.
        let mut connection = f.pool.acquire().await.unwrap();
        assert!(
            !set(
                &mut connection,
                &platform("platform-streamlink"),
                "platform-streamlink",
                &CredentialSelection::Inherit
            )
            .await
            .unwrap()
        );
        drop(connection);
        assert_eq!(f.rows().await, 0);

        let mut connection = f.pool.acquire().await.unwrap();
        for selection in [CredentialSelection::None, fixed(&other)] {
            assert!(
                set(&mut connection, &owner, "platform-streamlink", &selection)
                    .await
                    .unwrap()
            );
            drop(connection);
            assert_eq!(f.stored(&owner, "platform-streamlink").await, Some(selection));
            connection = f.pool.acquire().await.unwrap();
        }

        // A platform row written around the repository is ignored: a streamer
        // without its own selection is anonymous, one with a selection keeps it.
        clear_owner(&mut connection, &owner).await.unwrap();
        let id: i64 = sqlx::query_scalar("INSERT INTO credential_selections(platform_config_id, mode) VALUES ('platform-streamlink', 'fixed') RETURNING id")
            .fetch_one(&mut *connection).await.unwrap();
        sqlx::query("INSERT INTO credential_selection_members(selection_id, platform_config_id, position, profile_id) VALUES (?, 'platform-streamlink', 0, ?)")
            .bind(id).bind(&account).execute(&mut *connection).await.unwrap();
        let layers = load_for_streamer(&mut connection, &streamer.id, "platform-streamlink", None)
            .await
            .unwrap();
        assert_eq!(layers.len(), 1);
        assert!(
            crate::credentials::resolve_authentication("platform-streamlink", &layers)
                .unwrap()
                .is_none()
        );
        set(&mut connection, &owner, "platform-streamlink", &fixed(&other))
            .await
            .unwrap();
        let layers = load_for_streamer(&mut connection, &streamer.id, "platform-streamlink", None)
            .await
            .unwrap();
        let policy = crate::credentials::resolve_authentication("platform-streamlink", &layers)
            .unwrap()
            .unwrap();
        assert_eq!(policy.owner, owner);
    })
    .await
    .unwrap();
}
