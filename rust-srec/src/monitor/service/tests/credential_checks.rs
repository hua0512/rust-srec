use super::*;
use crate::credentials::{
    CredentialBinding, CredentialExecutionService, CredentialIdentity, CredentialMaterial,
    CredentialOwner, CredentialSelection, PoolStrategy, ResolvedCredentialPolicy,
};
use crate::database::repositories::{CredentialProfileRepository, SqlxCredentialStore};
use tokio_util::task::AbortOnDropHandle;

#[tokio::test]
async fn overlapping_discovery_queue_and_bound_polls_extract_only_equivalent_results_once() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "overlap", StreamerState::NotLive, 0, None).await;
        let profiles = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
        let owner = CredentialOwner::Platform {
            platform_id: "platform-twitch".into(),
        };
        let mut accounts = Vec::new();
        for label in ["A", "B", "C"] {
            accounts.push(
                profiles
                    .create(
                        &owner,
                        owner.id(),
                        label,
                        true,
                        &CredentialMaterial {
                            cookies: format!("account={label}"),
                            refresh_token: None,
                            access_token: None,
                            reauth_config: None,
                        },
                    )
                    .await
                    .unwrap(),
            );
        }
        let selection = CredentialSelection::Pool {
            credential_ids: accounts.iter().map(|account| account.id.clone()).collect(),
            strategy: PoolStrategy::RoundRobin,
            failover: true,
            max_attempts: 3,
        };
        sqlx::query("UPDATE platform_config SET credential_selection = ? WHERE id = ?")
            .bind(serde_json::to_string(&selection).unwrap())
            .bind(owner.id())
            .execute(&pool)
            .await
            .unwrap();
        let policy = ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap();
        let mut monitor = build_test_monitor(&pool).await;
        let admission = Arc::new(PlatformAdmission::new(
            crate::monitor::RateLimiterManager::with_config(crate::monitor::RateLimiterConfig {
                max_tokens: 100,
                initial_tokens: 100,
                refill_rate: 100.0,
            }),
        ));
        let refresh = Arc::new(
            CredentialRefreshService::new(Arc::new(SqlxCredentialStore::new(
                pool.clone(),
                pool.clone(),
            )))
            .with_admission(admission),
        );
        monitor.set_execution_service(Arc::new(CredentialExecutionService::new(profiles, refresh)));
        let (entered, mut calls) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let extraction_release = release.clone();
        let mut detector = StreamDetector::new();
        detector.test_extraction = Some(Arc::new(move |cookies| {
            let release = extraction_release.clone();
            let entered = entered.clone();
            Box::pin(async move {
                let cookies = cookies.unwrap();
                entered.send(cookies.clone()).unwrap();
                release.acquire().await.unwrap().forget();
                Ok(LiveStatus::Live {
                    credential_binding: None,
                    credential_snapshot: None,
                    title: cookies,
                    category: None,
                    started_at: None,
                    viewer_count: None,
                    avatar: None,
                    streams: vec![],
                    media_headers: None,
                    media_extras: None,
                    next_check_hint: None,
                    candidates: vec![],
                })
            })
        }));
        monitor.detector = Arc::new(detector);
        let monitor = Arc::new(monitor);
        let check = |purpose| {
            let monitor = monitor.clone();
            AbortOnDropHandle::new(tokio::spawn(async move {
                let metadata = monitor.streamer_manager.get_streamer("overlap").unwrap();
                monitor
                    .check_streamer_for(&metadata, purpose)
                    .await
                    .unwrap()
            }))
        };
        // Discovery is in flight with A when a session becomes pinned to B.
        let discovery = check(CredentialCheckPurpose::Discovery);
        assert_eq!(calls.recv().await.unwrap(), "account=A");
        insert_active_session(&pool, "session", "overlap").await;
        let binding = CredentialBinding {
            identity: CredentialIdentity::Profile {
                profile_id: accounts[1].id.clone(),
            },
            revision: 1,
            policy,
            epoch: 1,
        };
        sqlx::query("UPDATE live_sessions SET credential_binding = ? WHERE id = 'session'")
            .bind(serde_json::to_string(&binding).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let queued = check(CredentialCheckPurpose::QueueStart);
        assert_eq!(calls.recv().await.unwrap(), "account=B");
        let bound = check(CredentialCheckPurpose::BoundPoll);
        assert_eq!(calls.recv().await.unwrap(), "account=B");
        let another_poll = check(CredentialCheckPurpose::Discovery);
        // Wait for this caller to join the existing cell while every extraction
        // is parked. No timers or provider requests are needed to order the race.
        loop {
            let joined = monitor.in_flight.iter().any(|entry| {
                entry.key().purpose == CredentialCheckPurpose::BoundPoll
                    && Arc::strong_count(entry.value()) >= 3
            });
            if joined {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(calls.try_recv().is_err());
        release.add_permits(3);
        for (operation, expected) in [
            (discovery, "account=A"),
            (queued, "account=B"),
            (bound, "account=B"),
            (another_poll, "account=B"),
        ] {
            assert_eq!(operation.await.unwrap().title(), Some(expected));
        }
        assert!(calls.try_recv().is_err());
        // A new binding epoch cannot reuse the just-completed B poll.
        let mut next = binding;
        next.identity = CredentialIdentity::Profile {
            profile_id: accounts[2].id.clone(),
        };
        next.epoch += 1;
        sqlx::query("UPDATE live_sessions SET credential_binding = ? WHERE id = 'session'")
            .bind(serde_json::to_string(&next).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let changed = check(CredentialCheckPurpose::BoundPoll);
        assert_eq!(calls.recv().await.unwrap(), "account=C");
        release.add_permits(1);
        assert_eq!(changed.await.unwrap().title(), Some("account=C"));
        // A successful bound poll wakes a pending start even when its earlier
        // login notification raced a pipeline, or a cooldown simply expired.
        let (wake, mut wakes) = mpsc::unbounded_channel();
        monitor.set_pending_credential_recovery(Arc::new(move |streamer| {
            wake.send(streamer).unwrap();
            Box::pin(async {})
        }));
        release.add_permits(1);
        let metadata = monitor.streamer_manager.get_streamer("overlap").unwrap();
        assert!(monitor.check_streamer(&metadata).await.unwrap().is_live());
        assert_eq!(wakes.recv().await.unwrap(), "overlap");
        monitor.cancellation.cancel();
    })
    .await
    .unwrap();
}
