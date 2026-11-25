use super::*;
use crate::repositories::SessionTxOps;
use srec_core::domain::credential::{
    CredentialBinding, CredentialIdentity, ResolvedCredentialPolicy,
};
use srec_core::domain::proxy::{ProxyError, ProxyRoute};

#[tokio::test]
async fn session_binding_is_revision_and_epoch_checked() {
    let pool = crate::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    sqlx::query("INSERT INTO streamers(id,name,url,platform_config_id,state) VALUES ('recording','Recording','https://www.huya.com/recording','platform-huya','NOT_LIVE')").execute(&pool).await.unwrap();
    let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
    let owner = CredentialOwner::Platform {
        platform_id: "platform-huya".into(),
    };
    let profile = profiles
        .create(
            owner.id(),
            "A",
            true,
            &CredentialMaterial {
                cookies: "cookie=secret-sentinel".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            &ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let selection = CredentialSelection::Fixed {
        credential_id: profile.id.clone(),
    };
    crate::repositories::credential_selections::test_support::set_platform_selection(
        &pool,
        owner.id(),
        &selection,
    )
    .await;
    let session_id = "session-1";
    {
        let mut tx = begin_immediate(&pool).await.unwrap();
        SessionTxOps::create_session(
            &mut tx,
            session_id,
            "recording",
            "Recording",
            jiff::Timestamp::now(),
            "Live",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profile.id.clone(),
        },
        revision: 1,
        policy: ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap(),
        epoch: 0,
    };
    let mut forged = binding.clone();
    forged.identity = CredentialIdentity::Anonymous;
    forged.revision = 0;
    {
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            commit_session_binding(&mut conn, session_id, "recording", &forged, false)
                .await
                .is_err(),
            "anonymous identity cannot accompany fixed policy"
        );
    }
    let mut forged = binding.clone();
    forged.policy.selection = CredentialSelection::None;
    {
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            commit_session_binding(&mut conn, session_id, "recording", &forged, false)
                .await
                .is_err(),
            "copied generation cannot authorize altered policy content"
        );
    }
    {
        let mut tx = begin_immediate(&pool).await.unwrap();
        let committed = commit_session_binding(&mut tx, session_id, "recording", &binding, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(committed.epoch, 1);
    }
    let raw: String =
        sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = ?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut saved: CredentialBinding = serde_json::from_str(&raw).unwrap();
    assert_eq!(saved.epoch, 1);
    assert!(profiles.delete(&profile.id, 1).await.is_err());
    let updated = profiles
        .update(
            &profile.id,
            1,
            None,
            None,
            Some(&CredentialMaterial {
                cookies: "cookie=replaced".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            }),
            None,
        )
        .await
        .unwrap();
    {
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            commit_session_binding(&mut conn, session_id, "recording", &binding, false)
                .await
                .is_err()
        );
    }
    saved.revision = updated.revision as u64;
    let changed = {
        let mut tx = begin_immediate(&pool).await.unwrap();
        let changed = commit_session_binding(&mut tx, session_id, "recording", &saved, true)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        changed
    };
    assert_eq!(changed.epoch, 2);
    {
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            commit_session_binding(&mut conn, session_id, "recording", &saved, true)
                .await
                .is_err()
        );
    }
}

/// Weibo checks work without an account, so a recording under a fixed
/// selection may start without one when the account cannot be used.
#[tokio::test]
async fn a_fixed_selection_records_without_its_account_where_accounts_are_optional() {
    let pool = crate::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    sqlx::query("INSERT INTO streamers(id,name,url,platform_config_id,state) VALUES ('recording','Recording','https://weibo.com/u/1000000001','platform-weibo','NOT_LIVE')").execute(&pool).await.unwrap();
    let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
    let owner = CredentialOwner::Platform {
        platform_id: "platform-weibo".into(),
    };
    let profile = profiles
        .create(
            owner.id(),
            "A",
            true,
            &CredentialMaterial {
                cookies: "SUB=account-sub".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            &ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let selection = CredentialSelection::Fixed {
        credential_id: profile.id.clone(),
    };
    crate::repositories::credential_selections::test_support::set_platform_selection(
        &pool,
        owner.id(),
        &selection,
    )
    .await;
    let binding = CredentialBinding {
        identity: CredentialIdentity::Anonymous,
        revision: 0,
        policy: ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap(),
        epoch: 0,
    };
    let mut tx = begin_immediate(&pool).await.unwrap();
    SessionTxOps::create_session(
        &mut tx,
        "weibo-session",
        "recording",
        "Recording",
        jiff::Timestamp::now(),
        "Live",
    )
    .await
    .unwrap();
    let committed = commit_session_binding(&mut tx, "weibo-session", "recording", &binding, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(committed.epoch, 1);
    assert_eq!(committed.identity, CredentialIdentity::Anonymous);
}

#[tokio::test]
async fn cancelling_profile_request_after_commit_does_not_skip_publication() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let pool = crate::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::run_migrations(&pool).await.unwrap();
        let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
        let owner = CredentialOwner::Platform {
            platform_id: "platform-huya".into(),
        };
        let profile = profiles
            .create(
                owner.id(),
                "A",
                true,
                &CredentialMaterial {
                    cookies: "a=1".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &ProxyRoute::Inherit,
            )
            .await
            .unwrap();
        let (published, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        profiles.bind_publication(Arc::new(move |owner| {
            published.send(owner).unwrap();
        }));
        let gate = Arc::new(crate::committed_writer::CommitTestGate::default());
        *profiles.commit_gate.write() = Some(gate.clone());
        let task_repository = profiles.clone();
        let id = profile.id.clone();
        let caller = tokio::spawn(async move {
            task_repository
                .update(&id, 1, Some("Renamed"), None, None, None)
                .await
        });
        gate.started.notified().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        gate.release.notify_one();
        assert_eq!(receiver.recv().await.unwrap(), owner);
        assert_eq!(profiles.get(&profile.id).await.unwrap().label, "Renamed");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_account_route_is_kept_replaced_or_removed_and_starts_a_new_revision() {
    let pool = crate::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
    let material = CredentialMaterial {
        cookies: "cookie=account".into(),
        refresh_token: None,
        access_token: None,
        reauth_config: None,
    };
    let first =
        super::super::proxies::save_for_test(&pool, "first", "http://first.example:8080").await;
    let second =
        super::super::proxies::save_for_test(&pool, "second", "socks5h://second.example:1080")
            .await;
    let pinned = |id: &str| ProxyRoute::Proxy { id: id.to_owned() };
    let profile = profiles
        .create("platform-huya", "A", true, &material, &pinned(&first))
        .await
        .unwrap();
    assert_eq!(profile.route().unwrap(), pinned(&first));
    let summary = serde_json::to_value(profile.summary()).unwrap();
    assert_eq!(
        summary["proxy_route"],
        serde_json::json!({"kind": "proxy", "id": first})
    );

    let renamed = profiles
        .update(&profile.id, profile.version, Some("B"), None, None, None)
        .await
        .unwrap();
    assert_eq!(renamed.revision, profile.revision);
    assert_eq!(renamed.route().unwrap(), pinned(&first));
    let same = profiles
        .update(
            &profile.id,
            renamed.version,
            None,
            None,
            None,
            Some(&pinned(&first)),
        )
        .await
        .unwrap();
    assert_eq!(same.revision, profile.revision);

    let moved = profiles
        .update(
            &profile.id,
            same.version,
            None,
            None,
            None,
            Some(&pinned(&second)),
        )
        .await
        .unwrap();
    assert_eq!(moved.revision, profile.revision + 1);
    assert_eq!(moved.material().unwrap().cookies, "cookie=account");
    let direct = profiles
        .update(
            &profile.id,
            moved.version,
            None,
            None,
            None,
            Some(&ProxyRoute::Direct),
        )
        .await
        .unwrap();
    assert_eq!(direct.revision, moved.revision + 1);
    assert_eq!(direct.route().unwrap(), ProxyRoute::Direct);

    // A route naming no saved proxy is refused and changes nothing.
    assert!(matches!(
        profiles
            .update(
                &profile.id,
                direct.version,
                None,
                None,
                None,
                Some(&pinned("missing")),
            )
            .await,
        Err(Error::Proxy(ProxyError::Missing(_)))
    ));
    assert_eq!(
        profiles.get(&profile.id).await.unwrap().route().unwrap(),
        ProxyRoute::Direct
    );
}

#[tokio::test]
async fn changing_sites_republishes_the_platform_configuration() {
    let pool = crate::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
    let (published, mut configuration) = tokio::sync::mpsc::unbounded_channel();
    profiles.bind_publication(Arc::new(move |owner| {
        published.send(owner).unwrap();
    }));
    let (published, mut material) = tokio::sync::mpsc::unbounded_channel();
    profiles.bind_material_publication(Arc::new(move |owner| {
        published.send(owner).unwrap();
    }));
    let owner = CredentialOwner::Platform {
        platform_id: "platform-streamlink".into(),
    };
    let account = CredentialMaterial {
        cookies: "a=1".into(),
        refresh_token: None,
        access_token: None,
        reauth_config: None,
    };
    let profile = profiles
        .create(owner.id(), "A", true, &account, &ProxyRoute::Inherit)
        .await
        .unwrap();
    assert_eq!(material.try_recv().unwrap(), owner);
    let renamed = profiles
        .update(&profile.id, profile.version, Some("B"), None, None, None)
        .await
        .unwrap();
    assert_eq!(material.try_recv().unwrap(), owner);
    assert!(configuration.try_recv().is_err());

    let sites = ["kick.com".to_owned()];
    let named = profiles
        .edit(
            &renamed.id,
            renamed.version,
            ProfileEdit {
                sites: Some(&sites),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(configuration.try_recv().unwrap(), owner);
    assert_eq!(material.try_recv().unwrap(), owner);
    // Writing the same sites again changes nothing they decide.
    let same = profiles
        .edit(
            &named.id,
            named.version,
            ProfileEdit {
                sites: Some(&sites),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(configuration.try_recv().is_err());
    profiles.delete(&same.id, same.version).await.unwrap();
    assert_eq!(configuration.try_recv().unwrap(), owner);
}
