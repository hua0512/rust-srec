use super::*;
use crate::credentials::{CredentialBinding, CredentialIdentity, ResolvedCredentialPolicy};
use crate::database::repositories::{SessionLifecycleRepository, StartSessionInputs};

#[tokio::test]
async fn session_binding_and_outbox_are_atomic_revision_checked_and_secret_free() {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    sqlx::query("INSERT INTO streamers(id,name,url,platform_config_id,state) VALUES ('recording','Recording','https://www.huya.com/recording','platform-huya','NOT_LIVE')").execute(&pool).await.unwrap();
    let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
    let owner = CredentialOwner::Platform {
        platform_id: "platform-huya".into(),
    };
    let profile = profiles
        .create(
            &owner,
            owner.id(),
            "A",
            true,
            &CredentialMaterial {
                cookies: "cookie=secret-sentinel".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
        )
        .await
        .unwrap();
    let selection = CredentialSelection::Fixed {
        credential_id: profile.id.clone(),
    };
    sqlx::query("UPDATE platform_config SET credential_selection = ? WHERE id = ?")
        .bind(serde_json::to_string(&selection).unwrap())
        .bind(owner.id())
        .execute(&pool)
        .await
        .unwrap();
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profile.id.clone(),
        },
        revision: 1,
        policy: ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap(),
        epoch: 0,
    };
    let inputs = StartSessionInputs {
        credential_binding: Some(binding.clone()),
        streamer_id: "recording".into(),
        streamer_name: "Recording".into(),
        streamer_url: "https://www.huya.com/recording".into(),
        current_avatar: None,
        new_avatar: None,
        title: "Live".into(),
        category: None,
        streams: Vec::new(),
        media_headers: Some(std::collections::HashMap::from([(
            "Cookie".into(),
            "secret-sentinel".into(),
        )])),
        media_extras: Some(std::collections::HashMap::from([(
            "session_cookies".into(),
            "secret-sentinel".into(),
        )])),
        now: chrono::Utc::now(),
    };
    let lifecycle = SessionLifecycleRepository::new(pool.clone());
    let mut forged = inputs.clone();
    forged.credential_binding.as_mut().unwrap().identity = CredentialIdentity::Anonymous;
    forged.credential_binding.as_mut().unwrap().revision = 0;
    assert!(
        lifecycle.start_or_resume(forged).await.is_err(),
        "anonymous identity cannot accompany fixed policy"
    );
    let mut forged = inputs.clone();
    forged.credential_binding.as_mut().unwrap().policy.selection = CredentialSelection::None;
    assert!(
        lifecycle.start_or_resume(forged).await.is_err(),
        "copied generation cannot authorize altered policy content"
    );
    let outcome = lifecycle.start_or_resume(inputs.clone()).await.unwrap();
    let raw: String =
        sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = ?")
            .bind(outcome.session_id())
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut saved: CredentialBinding = serde_json::from_str(&raw).unwrap();
    assert_eq!(saved.epoch, 1);
    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM monitor_event_outbox WHERE streamer_id = 'recording'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!payload.contains("secret-sentinel"));
    assert!(payload.contains(&profile.id));
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
        )
        .await
        .unwrap();
    assert!(lifecycle.start_or_resume(inputs).await.is_err());
    saved.revision = updated.revision as u64;
    let changed = lifecycle
        .commit_credential_binding(
            outcome.session_id().into(),
            "recording".into(),
            saved.clone(),
        )
        .await
        .unwrap();
    assert_eq!(changed.epoch, 2);
    assert!(
        lifecycle
            .commit_credential_binding(outcome.session_id().into(), "recording".into(), saved)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM monitor_event_outbox WHERE streamer_id='recording'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "stale handoff must roll back all session and outbox changes"
    );
}

#[tokio::test]
async fn cancelling_profile_request_after_commit_does_not_skip_publication() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
        let owner = CredentialOwner::Platform {
            platform_id: "platform-huya".into(),
        };
        let profile = profiles
            .create(
                &owner,
                owner.id(),
                "A",
                true,
                &CredentialMaterial {
                    cookies: "a=1".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
            )
            .await
            .unwrap();
        let (published, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        profiles.bind_publication(Arc::new(move |owner| {
            published.send(owner).unwrap();
        }));
        let gate = Arc::new(crate::database::committed_writer::CommitTestGate::default());
        *profiles.commit_gate.write() = Some(gate.clone());
        let task_repository = profiles.clone();
        let id = profile.id.clone();
        let caller = tokio::spawn(async move {
            task_repository
                .update(&id, 1, Some("Renamed"), None, None)
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
