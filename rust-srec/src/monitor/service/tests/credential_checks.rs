use super::*;
use crate::credentials::{
    CredentialBinding, CredentialExecutionService, CredentialIdentity, CredentialMaterial,
    CredentialOwner, CredentialProviderRegistry, CredentialSelection, PoolStrategy,
    ResolvedCredentialPolicy,
};
use crate::database::repositories::CredentialProfileRepository;
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
                        owner.id(),
                        label,
                        true,
                        &CredentialMaterial {
                            cookies: format!("account={label}"),
                            refresh_token: None,
                            access_token: None,
                            reauth_config: None,
                        },
                        &crate::proxies::ProxyRoute::Inherit,
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
        crate::database::repositories::credential_selections::test_support::set_platform_selection(
            &pool,
            owner.id(),
            &selection,
        )
        .await;
        let policy = ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap();
        let mut monitor = build_test_monitor(&pool).await;
        let refresh = Arc::new(
            CredentialProviderRegistry::new()
                .with_admission(crate::credentials::test_support::unthrottled_admission()),
        );
        monitor.set_execution_service(Arc::new(CredentialExecutionService::new(profiles, refresh)));
        let (entered, mut calls) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let extraction_release = release.clone();
        let mut detector = StreamDetector::new();
        detector.test_extraction = Some(Arc::new(move |cookies, _proxy| {
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
        // login notification raced a pipeline.
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

#[tokio::test]
async fn a_check_with_an_account_uses_the_accounts_own_route() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "proxied", StreamerState::NotLive, 0, None).await;
        let profiles = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
        let account = profiles
            .create(
                "platform-twitch",
                "A",
                true,
                &CredentialMaterial {
                    cookies: "account=A".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &crate::proxies::ProxyRoute::Proxy {
                    id: crate::database::repositories::proxies::save_for_test(
                        &pool,
                        "account",
                        "socks5h://account-proxy.example:1080",
                    )
                    .await,
                },
            )
            .await
            .unwrap();
        crate::database::repositories::credential_selections::test_support::set_platform_selection(
            &pool,
            "platform-twitch",
            &CredentialSelection::Fixed {
                credential_id: account.id,
            },
        )
        .await;
        let mut monitor = build_test_monitor(&pool).await;
        let refresh = Arc::new(
            CredentialProviderRegistry::new()
                .with_admission(crate::credentials::test_support::unthrottled_admission()),
        );
        monitor.set_execution_service(Arc::new(CredentialExecutionService::new(profiles, refresh)));
        let (used, mut proxies) = mpsc::unbounded_channel();
        let mut detector = StreamDetector::new();
        detector.test_extraction = Some(Arc::new(move |_cookies, proxy| {
            used.send(proxy).unwrap();
            Box::pin(async { Ok(LiveStatus::Offline) })
        }));
        monitor.detector = Arc::new(detector);
        let metadata = monitor.streamer_manager.get_streamer("proxied").unwrap();
        monitor.check_streamer(&metadata).await.unwrap();
        let proxy = proxies.recv().await.unwrap();
        assert_eq!(
            proxy.endpoint().map(|endpoint| endpoint.url.as_str()),
            Some("socks5h://account-proxy.example:1080")
        );
        monitor.cancellation.cancel();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_check_without_an_account_uses_the_streamers_route() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "anonymous", StreamerState::NotLive, 0, None).await;
        let platform_proxy = crate::database::repositories::proxies::save_for_test(
            &pool,
            "platform",
            "http://platform-proxy.example:3128",
        )
        .await;
        crate::database::repositories::proxies::set_route(
            &mut pool.acquire().await.unwrap(),
            &crate::database::repositories::proxies::RouteOwner::Platform("platform-twitch".into()),
            &crate::proxies::ProxyRoute::Proxy { id: platform_proxy },
        )
        .await
        .unwrap();
        let mut monitor = build_test_monitor(&pool).await;
        let (used, mut proxies) = mpsc::unbounded_channel();
        let mut detector = StreamDetector::new();
        detector.test_extraction = Some(Arc::new(move |_cookies, proxy| {
            used.send(proxy).unwrap();
            Box::pin(async { Ok(LiveStatus::Offline) })
        }));
        monitor.detector = Arc::new(detector);
        let metadata = monitor.streamer_manager.get_streamer("anonymous").unwrap();
        monitor.check_streamer(&metadata).await.unwrap();
        let proxy = proxies.recv().await.unwrap();
        assert_eq!(
            proxy.endpoint().map(|endpoint| endpoint.url.as_str()),
            Some("http://platform-proxy.example:3128")
        );
        monitor.cancellation.cancel();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn only_unavailable_accounts_block_a_streamer_and_reaching_an_account_lifts_it() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "blocked", StreamerState::NotLive, 0, None).await;
        let profiles = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
        let account = profiles
            .create(
                "platform-twitch",
                "A",
                true,
                &CredentialMaterial {
                    cookies: "account=A".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap();
        crate::database::repositories::credential_selections::test_support::set_platform_selection(
            &pool,
            "platform-twitch",
            &CredentialSelection::Fixed {
                credential_id: account.id.clone(),
            },
        )
        .await;
        let mut monitor = build_test_monitor(&pool).await;
        let refresh = Arc::new(
            CredentialProviderRegistry::new()
                .with_admission(crate::credentials::test_support::unthrottled_admission()),
        );
        monitor.set_execution_service(Arc::new(CredentialExecutionService::new(
            profiles.clone(),
            refresh,
        )));
        let unreachable = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let extraction_fails = unreachable.clone();
        let mut detector = StreamDetector::new();
        detector.test_extraction = Some(Arc::new(move |_cookies, _proxy| {
            let fails = extraction_fails.load(std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if fails {
                    Err(Error::Monitor("platform unreachable".into()))
                } else {
                    Ok(LiveStatus::Offline)
                }
            })
        }));
        monitor.detector = Arc::new(detector);
        let blocks = monitor.credential_blocks().clone();
        let mut changes = blocks.subscribe();
        let metadata = monitor.streamer_manager.get_streamer("blocked").unwrap();
        let set_enabled = |version: i64, enabled: bool| {
            let profiles = profiles.clone();
            let id = account.id.clone();
            async move {
                profiles
                    .update(&id, version, None, Some(enabled), None, None)
                    .await
                    .unwrap()
                    .version
            }
        };

        // A failure after the account was handed over is an ordinary error.
        assert!(matches!(
            monitor.check_streamer(&metadata).await,
            Err(Error::Monitor(_))
        ));
        assert!(blocks.get("blocked").is_none());

        let version = set_enabled(account.version, false).await;
        assert!(matches!(
            monitor.check_streamer(&metadata).await,
            Err(Error::CredentialUnavailable(_))
        ));
        let block = blocks.get("blocked").expect("no usable account blocks");
        assert_eq!(
            block.reason,
            crate::credentials::UnavailableReason::ProfilesDisabled
        );
        assert_eq!(block.platform_id, "platform-twitch");
        monitor.check_streamer(&metadata).await.unwrap_err();
        assert_eq!(blocks.get("blocked").unwrap().since, block.since);

        // Re-enabled: the check reaches the account, so even its failure lifts
        // the block.
        let version = set_enabled(version, true).await;
        assert!(matches!(
            monitor.check_streamer(&metadata).await,
            Err(Error::Monitor(_))
        ));
        assert!(blocks.get("blocked").is_none());

        let version = set_enabled(version, false).await;
        monitor.check_streamer(&metadata).await.unwrap_err();
        assert!(blocks.get("blocked").is_some());
        set_enabled(version, true).await;
        unreachable.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            monitor.check_streamer(&metadata).await,
            Ok(LiveStatus::Offline)
        ));
        assert!(blocks.get("blocked").is_none());

        let published: Vec<_> = std::iter::from_fn(|| changes.try_recv().ok())
            .map(|change| (change.streamer_id, change.block.is_some()))
            .collect();
        assert_eq!(
            published,
            [true, false, true, false].map(|blocked| ("blocked".to_string(), blocked))
        );
        monitor.cancellation.cancel();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_streamer_without_any_account_selection_is_never_blocked() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "anonymous", StreamerState::NotLive, 0, None).await;
        let mut monitor = build_test_monitor(&pool).await;
        let mut detector = StreamDetector::new();
        detector.test_extraction = Some(Arc::new(|_cookies, _proxy| {
            Box::pin(async { Ok(LiveStatus::Offline) })
        }));
        monitor.detector = Arc::new(detector);
        // A block left from before the selection was removed.
        monitor.credential_blocks().block(
            "anonymous",
            "platform-twitch",
            crate::credentials::UnavailableReason::LoginRequired,
        );
        let metadata = monitor.streamer_manager.get_streamer("anonymous").unwrap();
        monitor.check_streamer(&metadata).await.unwrap();
        assert!(monitor.credential_blocks().get("anonymous").is_none());
        monitor.cancellation.cancel();
    })
    .await
    .unwrap();
}
