use super::*;
use crate::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
    CredentialSelection, CredentialSnapshot, ResolvedCredentialPolicy,
};
use crate::database::repositories::CredentialProfileRepository;
use crate::database::repositories::credential_selections::test_support::set_platform_selection;

fn bound_profile(binding: &CredentialBinding) -> &str {
    let CredentialIdentity::Profile { profile_id } = &binding.identity else {
        panic!("expected a profile binding, got {:?}", binding.identity)
    };
    profile_id
}

async fn managed(fixture: &Fixture) -> (String, CredentialBinding, CredentialProfileRepository) {
    let repository = CredentialProfileRepository::new(fixture.pool.clone(), fixture.pool.clone());
    let owner = CredentialOwner::Platform {
        platform_id: "platform-twitch".into(),
    };
    let profile = repository
        .create(
            owner.id(),
            "Recording account",
            true,
            &CredentialMaterial {
                cookies: "session=selected".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            &crate::proxies::ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let selection = CredentialSelection::Fixed {
        credential_id: profile.id.clone(),
    };
    set_platform_selection(&fixture.pool, "platform-twitch", &selection).await;
    let session = fixture.live().await;
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profile.id,
        },
        revision: 1,
        policy: ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap(),
        epoch: 0,
    };
    let binding = fixture
        .coordinator
        .session_lifecycle
        .commit_credential_binding(&session, STREAMER, binding)
        .await
        .unwrap();
    (session, binding, repository)
}

#[tokio::test]
async fn managed_startup_replaces_complete_bundle_and_shares_one_snapshot_with_danmu() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let account_proxy = crate::database::repositories::proxies::create(
            &mut fixture.pool.acquire().await.unwrap(),
            None,
            "account",
            &crate::proxies::ProxyEndpoint::new(
                "http://account-proxy.example:8080",
                Some(("user".into(), "secret".into())),
            ),
        )
        .await
        .unwrap();
        let profile = repository
            .update(
                profile_id,
                1,
                None,
                None,
                Some(&CredentialMaterial {
                    cookies: "session=reactive-new".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                }),
                Some(&crate::proxies::ProxyRoute::Proxy {
                    id: account_proxy.id.clone(),
                }),
            )
            .await
            .unwrap();
        let route = repository.account_route(&profile, None).await.unwrap();
        let mut fresh_binding = binding.clone();
        fresh_binding.revision = profile.revision as u64;
        let mut fresh = fresh_live();
        if let crate::monitor::LiveStatus::Live {
            credential_binding,
            credential_snapshot,
            media_headers,
            ..
        } = &mut fresh
        {
            *credential_binding = Some(Box::new(fresh_binding.clone()));
            *credential_snapshot = Some(Arc::new(CredentialSnapshot {
                binding: fresh_binding,
                material: profile.material().unwrap(),
                route,
            }));
            media_headers.as_mut().unwrap().insert(
                "cOoKiE".into(),
                "session=must-not-win; device=anonymous".into(),
            );
        }
        let checker = FreshnessGate::new(Ok(fresh));
        checker.release.add_permits(1);
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker.clone());
        let mut handoff = payload(&session);
        handoff.credential_binding = Some(binding);
        handoff.streams.clear();
        run_live_download_pipeline(fixture.coordinator.clone(), handoff, false).await;
        fixture.engine.started.notified().await;
        assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
        let handle = fixture.engine.handles.lock()[0].clone();
        let config = handle.config_snapshot();
        assert_eq!(
            config.cookies.as_deref(),
            Some("session=reactive-new; device=anonymous")
        );
        // The download leaves through the account's proxy, like its extraction.
        let account_endpoint = crate::proxies::ProxyEndpoint::new(
            "http://account-proxy.example:8080",
            Some(("user".into(), "secret".into())),
        );
        assert_eq!(
            config.proxy,
            crate::proxies::ProxyTarget::Explicit(account_endpoint.clone())
        );
        assert!(config.url.contains("fresh"));
        assert!(
            config
                .headers
                .iter()
                .all(|(key, _)| !key.eq_ignore_ascii_case("cookie"))
        );
        assert_eq!(
            fixture.danmu_configs.lock()[0].cookies.as_deref(),
            Some("session=reactive-new; device=anonymous")
        );
        // Danmu leaves through the same proxy, with the same login.
        assert_eq!(
            fixture.danmu_configs.lock()[0].proxy,
            Some(platforms_parser::danmaku::DanmuProxy::new(account_endpoint).unwrap())
        );
        let current = fixture
            .coordinator
            .stream_monitor
            .session_credential_binding(&session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.epoch, 2);
        assert_eq!(current.revision, 2);
        assert!(
            fixture.coordinator.download_manager.get_active_downloads()[0]
                .url
                .is_empty()
        );
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stale_epoch_handoff_cannot_start_or_replace_newer_binding() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, stale, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&stale);
        let profile = repository
            .update(
                profile_id,
                1,
                None,
                None,
                Some(&CredentialMaterial {
                    cookies: "session=new".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                }),
                None,
            )
            .await
            .unwrap();
        let mut updated = stale.clone();
        updated.revision = profile.revision as u64;
        fixture
            .coordinator
            .session_lifecycle
            .commit_credential_binding(&session, STREAMER, updated)
            .await
            .unwrap();
        let checker = FreshnessGate::new(Ok(fresh_live()));
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker.clone());
        let mut handoff = payload(&session);
        handoff.credential_binding = Some(stale);
        run_live_download_pipeline(fixture.coordinator.clone(), handoff, false).await;
        fixture.assert_no_start();
        assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn managed_queue_failure_never_uses_cached_urls_or_ends_the_live_session() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, _) = managed(&fixture).await;
        let checker = FreshnessGate::new(Err(crate::Error::CredentialUnavailable(
            crate::credentials::CredentialUnavailable {
                reason: crate::credentials::UnavailableReason::LoginRequired,
                policy_generation: binding.policy.generation.clone(),
            },
        )));
        checker.release.add_permits(1);
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker);
        let mut handoff = payload(&session);
        handoff.credential_binding = Some(binding);
        run_live_download_pipeline(fixture.coordinator.clone(), handoff, true).await;
        fixture.assert_no_start();
        assert!(
            fixture
                .coordinator
                .session_lifecycle
                .is_session_active(&session)
        );
        assert_eq!(
            fixture
                .coordinator
                .streamer_manager
                .get_streamer(STREAMER)
                .unwrap()
                .consecutive_error_count,
            0
        );
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn legacy_replayed_handoff_requires_fresh_media_even_without_queue_wait() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for succeeds in [false, true] {
            let mut fixture = Fixture::new().await;
            let session = fixture.live().await;
            let checker = FreshnessGate::new(if succeeds {
                Ok(fresh_live())
            } else {
                Err(crate::Error::Monitor("diagnostic unavailable".into()))
            });
            checker.release.add_permits(1);
            Arc::get_mut(&mut fixture.coordinator)
                .unwrap()
                .freshness_check = Some(checker.clone());
            let mut handoff = payload(&session);
            handoff.runtime_instance = None;
            run_live_download_pipeline(fixture.coordinator.clone(), handoff, false).await;
            assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
            if succeeds {
                fixture.engine.started.notified().await;
                assert!(
                    fixture.engine.handles.lock()[0]
                        .config
                        .read()
                        .url
                        .contains("fresh")
                );
            } else {
                fixture.assert_no_start();
            }
            fixture.close().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn pending_credential_start_recovers_with_next_account_in_the_same_session() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let repository =
            CredentialProfileRepository::new(fixture.pool.clone(), fixture.pool.clone());
        let owner = CredentialOwner::Platform {
            platform_id: "platform-twitch".into(),
        };
        let mut profiles = Vec::new();
        for account in ["A", "B", "C"] {
            profiles.push(
                repository
                    .create(
                        owner.id(),
                        account,
                        account != "C",
                        &CredentialMaterial {
                            cookies: format!("account={account}"),
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
            credential_ids: profiles.iter().map(|profile| profile.id.clone()).collect(),
            strategy: crate::credentials::PoolStrategy::RoundRobin,
            failover: true,
            max_attempts: 3,
        };
        set_platform_selection(&fixture.pool, "platform-twitch", &selection).await;
        let session = fixture.live().await;
        let policy = ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap();
        let binding = fixture
            .coordinator
            .session_lifecycle
            .commit_credential_binding(
                &session,
                STREAMER,
                CredentialBinding {
                    identity: CredentialIdentity::Profile {
                        profile_id: profiles[1].id.clone(),
                    },
                    revision: 1,
                    policy: policy.clone(),
                    epoch: 0,
                },
            )
            .await
            .unwrap();
        repository
            .publish_health(
                &profiles[1],
                crate::credentials::CredentialValidity::Invalid,
                Some(crate::credentials::HealthReason::LoginRequired),
                false,
            )
            .await
            .unwrap();
        let checker = FreshnessGate::new(Err(crate::Error::CredentialUnavailable(
            crate::credentials::CredentialUnavailable {
                reason: crate::credentials::UnavailableReason::AttemptsExhausted,
                policy_generation: policy.generation.clone(),
            },
        )));
        checker.release.add_permits(1);
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker.clone());
        fixture
            .coordinator
            .resume_pending_credentials(STREAMER)
            .await;
        while checker.calls.load(Ordering::SeqCst) == 0
            || fixture.coordinator.pending_pipelines.contains_key(STREAMER)
        {
            tokio::task::yield_now().await;
        }
        fixture.assert_no_start();
        assert!(
            fixture
                .coordinator
                .session_lifecycle
                .is_session_active(&session)
        );
        while Arc::strong_count(&fixture.coordinator) != 1 {
            tokio::task::yield_now().await;
        }
        let restarted_lifecycle = Arc::new(SessionLifecycle::new(
            Arc::new(SessionLifecycleRepository::new(fixture.pool.clone())),
            Arc::new(OfflineClassifier::new()),
            64,
        ));
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .session_lifecycle = restarted_lifecycle;
        assert!(
            !fixture
                .coordinator
                .session_lifecycle
                .is_session_active(&session),
            "restart begins without an in-memory recording pin"
        );
        let recovered = repository
            .update(&profiles[2].id, 1, None, Some(true), None, None)
            .await
            .unwrap();
        let fresh_binding = CredentialBinding {
            identity: CredentialIdentity::Profile {
                profile_id: recovered.id.clone(),
            },
            revision: recovered.revision as u64,
            policy,
            epoch: binding.epoch,
        };
        let mut status = fresh_live();
        if let crate::monitor::LiveStatus::Live {
            credential_binding,
            credential_snapshot,
            ..
        } = &mut status
        {
            *credential_binding = Some(Box::new(fresh_binding.clone()));
            *credential_snapshot = Some(Arc::new(CredentialSnapshot {
                binding: fresh_binding,
                material: recovered.material().unwrap(),
                route: crate::proxies::ResolvedRoute::default(),
            }));
        }
        *checker.result.lock() = Some(Ok(status));
        checker.release.add_permits(1);
        fixture
            .coordinator
            .resume_pending_credentials(STREAMER)
            .await;
        fixture.engine.started.notified().await;
        while fixture.coordinator.pending_pipelines.contains_key(STREAMER) {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            fixture.engine.handles.lock()[0].config.read().session_id,
            session
        );
        assert_eq!(
            fixture.engine.handles.lock()[0]
                .config
                .read()
                .cookies
                .as_deref(),
            Some("account=C")
        );
        assert_eq!(
            fixture
                .coordinator
                .stream_monitor
                .session_credential_binding(&session)
                .await
                .unwrap()
                .unwrap()
                .epoch,
            2
        );
        assert!(
            fixture
                .coordinator
                .session_lifecycle
                .is_session_active(&session),
            "fresh bound startup hydrates the original committed session"
        );
        assert_eq!(
            fixture
                .coordinator
                .streamer_manager
                .get_streamer(STREAMER)
                .unwrap()
                .consecutive_error_count,
            0
        );
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn pending_credential_recovery_leaves_hysteresis_to_live_detection() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, _, _) = managed(&fixture).await;
        let checker = FreshnessGate::new(Ok(fresh_live()));
        checker.release.add_permits(1);
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker.clone());
        fixture
            .coordinator
            .session_lifecycle
            .on_download_terminal(&crate::downloader::DownloadTerminalEvent::Completed {
                download_id: "ended-attempt".into(),
                streamer_id: STREAMER.into(),
                streamer_name: "Coordinator".into(),
                session_id: session.clone(),
                total_bytes: 0,
                total_duration_secs: 0.0,
                total_segments: 0,
                file_path: None,
                engine_signal: EngineEndSignal::CleanDisconnect,
                stop_cause: None,
            })
            .await
            .unwrap();
        let in_hysteresis = || {
            fixture
                .coordinator
                .session_lifecycle
                .session_snapshot(&session)
                .is_some_and(|state| state.is_hysteresis())
        };
        assert!(in_hysteresis());
        // Only live detection cancels the quiet-period timer. A direct download
        // start would leave that timer free to end the session under the engine.
        fixture
            .coordinator
            .resume_pending_credentials(STREAMER)
            .await;
        // Startup runs asynchronously; give a wrongly started pipeline time to
        // reach its freshness check before asserting that none began.
        assert!(
            tokio::time::timeout(Duration::from_millis(500), checker.entered.notified())
                .await
                .is_err(),
            "no download startup may begin while the session is in hysteresis"
        );
        assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
        fixture.assert_no_start();
        assert!(in_hysteresis());
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn converted_legacy_session_resumes_with_a_first_committed_binding() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        // The session starts under legacy configuration, so it has no binding.
        let session = fixture.live().await;
        let repository =
            CredentialProfileRepository::new(fixture.pool.clone(), fixture.pool.clone());
        let owner = CredentialOwner::Platform {
            platform_id: "platform-twitch".into(),
        };
        let profile = repository
            .create(
                owner.id(),
                "Converted account",
                true,
                &CredentialMaterial {
                    cookies: "session=converted".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap();
        let selection = CredentialSelection::Fixed {
            credential_id: profile.id.clone(),
        };
        set_platform_selection(&fixture.pool, "platform-twitch", &selection).await;
        let binding = CredentialBinding {
            identity: CredentialIdentity::Profile {
                profile_id: profile.id.clone(),
            },
            revision: profile.revision as u64,
            policy: ResolvedCredentialPolicy::new(owner.id().into(), owner, selection).unwrap(),
            epoch: 0,
        };
        let mut status = fresh_live();
        if let crate::monitor::LiveStatus::Live {
            credential_binding,
            credential_snapshot,
            ..
        } = &mut status
        {
            *credential_binding = Some(Box::new(binding.clone()));
            *credential_snapshot = Some(Arc::new(CredentialSnapshot {
                binding: binding.clone(),
                material: profile.material().unwrap(),
                route: crate::proxies::ResolvedRoute::default(),
            }));
        }
        let checker = FreshnessGate::new(Ok(status));
        checker.release.add_permits(1);
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker.clone());
        // A hysteresis resume hands off the monitor's uncommitted binding.
        let mut handoff = payload(&session);
        handoff.credential_binding = Some(binding);
        run_live_download_pipeline(fixture.coordinator.clone(), handoff, false).await;
        fixture.engine.started.notified().await;
        assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.engine.handles.lock()[0]
                .config
                .read()
                .cookies
                .as_deref(),
            Some("session=converted")
        );
        assert_eq!(
            fixture
                .coordinator
                .stream_monitor
                .session_credential_binding(&session)
                .await
                .unwrap()
                .unwrap()
                .epoch,
            1
        );
        fixture.close().await;
    })
    .await
    .unwrap();
}

fn bound_live(
    binding: &CredentialBinding,
    profile: &crate::credentials::CredentialProfile,
    label: &str,
) -> crate::monitor::LiveStatus {
    let mut status = fresh_live();
    if let crate::monitor::LiveStatus::Live {
        credential_binding,
        credential_snapshot,
        streams,
        ..
    } = &mut status
    {
        streams[0].url = format!("https://example.invalid/{label}.flv");
        *credential_binding = Some(Box::new(binding.clone()));
        *credential_snapshot = Some(Arc::new(CredentialSnapshot {
            binding: binding.clone(),
            material: profile.material().unwrap(),
            route: crate::proxies::ResolvedRoute::default(),
        }));
    }
    status
}

/// Answers every check with the same bound result.
struct RepeatingCheck {
    calls: AtomicUsize,
    status: crate::monitor::LiveStatus,
}

struct ChangedThenLive(RepeatingCheck);

#[async_trait]
impl FreshnessCheck for ChangedThenLive {
    async fn check(
        &self,
        _metadata: &crate::streamer::StreamerMetadata,
    ) -> crate::Result<crate::monitor::LiveStatus> {
        if self.0.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(crate::credentials::ProfileError::SourceChanged.into())
        } else {
            Ok(self.0.status.clone())
        }
    }
}

#[tokio::test]
async fn login_racing_startup_extraction_retries_once_without_another_config_event() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let profile = repository.get(profile_id).await.unwrap();
        let check = Arc::new(ChangedThenLive(RepeatingCheck {
            calls: AtomicUsize::new(0),
            status: bound_live(&binding, &profile, "after-race"),
        }));
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(check.clone());
        fixture
            .coordinator
            .resume_pending_credentials(STREAMER)
            .await;
        wait_for_start(&fixture).await;
        assert_eq!(check.0.calls.load(Ordering::SeqCst), 2);
        let config = fixture.engine.handles.lock()[0].config_snapshot();
        assert_eq!(config.session_id, session);
        assert!(config.url.contains("after-race"));
        assert_eq!(fixture.coordinator.download_manager.active_count(), 1);
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[async_trait]
impl FreshnessCheck for RepeatingCheck {
    async fn check(
        &self,
        _metadata: &crate::streamer::StreamerMetadata,
    ) -> crate::Result<crate::monitor::LiveStatus> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.status.clone())
    }
}

async fn wait_for_start(fixture: &Fixture) {
    fixture.engine.started.notified().await;
    while fixture.coordinator.pending_pipelines.contains_key(STREAMER) {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn mesio_auth_failure_runs_one_diagnostic_whose_media_renews_the_next_attempt() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let profile = repository.get(profile_id).await.unwrap();
        let diagnostic = FreshnessGate::new(Ok(bound_live(&binding, &profile, "diagnostic")));
        diagnostic.release.add_permits(1);
        let startup = FreshnessGate::new(Err(crate::Error::Monitor(
            "startup must reuse the diagnostic bundle".into(),
        )));
        startup.release.add_permits(1);
        {
            let coordinator = Arc::get_mut(&mut fixture.coordinator).unwrap();
            coordinator.recovery_check = Some(diagnostic.clone());
            coordinator.freshness_check = Some(startup.clone());
        }
        let failed = crate::downloader::DownloadTerminalEvent::Failed {
            download_id: "attempt-1".into(),
            streamer_id: STREAMER.into(),
            streamer_name: "Coordinator".into(),
            session_id: session.clone(),
            engine_type: EngineType::Mesio,
            protocol: crate::downloader::DownloadProtocol::Flv,
            kind: crate::downloader::DownloadFailureKind::HttpClientError { status: 403 },
            error: "HTTP 403".into(),
            recoverable: false,
        };
        assert_eq!(
            fixture
                .coordinator
                .diagnose_credential_attempt(failed.clone())
                .await,
            Some(crate::downloader::DownloadFailureKind::CredentialRecovery)
        );
        // A repeated callback for the same attempt cannot run another diagnostic.
        assert_eq!(
            fixture
                .coordinator
                .diagnose_credential_attempt(failed)
                .await,
            None
        );
        assert_eq!(diagnostic.calls.load(Ordering::SeqCst), 1);
        // The status code alone never marks the account invalid.
        assert!(repository.health(profile_id).await.unwrap().is_none_or(
            |health| health.validity != crate::credentials::CredentialValidity::Invalid
        ));

        let mut handoff = payload(&session);
        handoff.credential_binding = Some(binding);
        run_live_download_pipeline(fixture.coordinator.clone(), handoff, false).await;
        fixture.engine.started.notified().await;
        assert_eq!(startup.calls.load(Ordering::SeqCst), 0);
        let config = fixture.engine.handles.lock()[0].config_snapshot();
        assert!(config.url.contains("diagnostic"), "{}", config.url);
        assert_eq!(config.cookies.as_deref(), Some("session=selected"));
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_start_rejected_by_a_changed_account_re_extracts_once() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let profile = repository.get(profile_id).await.unwrap();
        let check = Arc::new(RepeatingCheck {
            calls: AtomicUsize::new(0),
            status: bound_live(&binding, &profile, "fresh"),
        });
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(check.clone());
        let validations = Arc::new(AtomicUsize::new(0));
        let counted = validations.clone();
        fixture
            .coordinator
            .download_manager
            .set_credential_start_validator(Arc::new(move |_, _, _| {
                let first = counted.fetch_add(1, Ordering::SeqCst) == 0;
                Box::pin(async move {
                    if first {
                        Err(crate::credentials::ProfileError::SourceChanged.into())
                    } else {
                        Ok(())
                    }
                })
            }));
        let handoff = payload(&session);
        fixture
            .coordinator
            .handle_monitor_event(
                MonitorEvent::StreamerLive {
                    runtime_instance: handoff.runtime_instance,
                    credential_binding: Some(Box::new(binding)),
                    streamer_id: handoff.streamer_id,
                    session_id: handoff.session_id,
                    streamer_name: handoff.streamer_name,
                    streamer_url: handoff.streamer_url,
                    title: handoff.title,
                    category: None,
                    streams: handoff.streams,
                    media_headers: handoff.media_headers,
                    media_extras: None,
                    timestamp: Utc::now(),
                },
                false,
            )
            .await;
        wait_for_start(&fixture).await;
        assert_eq!(validations.load(Ordering::SeqCst), 2);
        assert_eq!(
            check.calls.load(Ordering::SeqCst),
            2,
            "one fresh bound extraction replaces the media of the rejected start"
        );
        assert_eq!(fixture.coordinator.download_manager.active_count(), 1);
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_credential_change_restarts_only_the_recordings_of_its_owner() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (_session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let profile = repository.get(profile_id).await.unwrap();
        let check = Arc::new(RepeatingCheck {
            calls: AtomicUsize::new(0),
            status: bound_live(&binding, &profile, "fresh"),
        });
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(check.clone());
        fixture
            .coordinator
            .resume_pending_credentials_for_owner(&CredentialOwner::Platform {
                platform_id: "platform-huya".into(),
            })
            .await;
        tokio::task::yield_now().await;
        assert!(fixture.coordinator.pending_pipelines.is_empty());
        assert_eq!(check.calls.load(Ordering::SeqCst), 0);
        fixture
            .coordinator
            .resume_pending_credentials_for_owner(&CredentialOwner::Platform {
                platform_id: "platform-twitch".into(),
            })
            .await;
        wait_for_start(&fixture).await;
        assert_eq!(check.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.engine.handles.lock()[0]
                .config
                .read()
                .cookies
                .as_deref(),
            Some("session=selected")
        );
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn managed_hysteresis_resume_keeps_the_binding_and_extracts_fresh_media() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let profile = repository.get(profile_id).await.unwrap();
        let checker = FreshnessGate::new(Ok(bound_live(&binding, &profile, "fresh")));
        checker.release.add_permits(1);
        Arc::get_mut(&mut fixture.coordinator)
            .unwrap()
            .freshness_check = Some(checker.clone());
        fixture
            .coordinator
            .session_lifecycle
            .on_download_terminal(&crate::downloader::DownloadTerminalEvent::Completed {
                download_id: "ended-attempt".into(),
                streamer_id: STREAMER.into(),
                streamer_name: "Coordinator".into(),
                session_id: session.clone(),
                total_bytes: 0,
                total_duration_secs: 0.0,
                total_segments: 0,
                file_path: None,
                engine_signal: EngineEndSignal::CleanDisconnect,
                stop_cause: None,
            })
            .await
            .unwrap();
        let cached = streams("cached");
        let resumed = fixture
            .coordinator
            .session_lifecycle
            .on_live_detected(LiveDetectedArgs {
                credential_binding: Some(&binding),
                streamer_id: STREAMER,
                streamer_name: "Coordinator",
                streamer_url: FakeProvider::URL,
                current_avatar: None,
                new_avatar: None,
                title: "Contract",
                category: None,
                streams: &cached,
                media_headers: None,
                media_extras: None,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert_eq!(resumed.session_id(), session);
        fixture
            .coordinator
            .handle_session_transition(SessionTransition::Started {
                session_id: session.clone(),
                streamer_id: STREAMER.into(),
                streamer_name: "Coordinator".into(),
                title: "Contract".into(),
                category: None,
                started_at: Utc::now(),
                from_hysteresis: true,
                download_start: Some(Box::new(crate::session::DownloadStartPayload {
                    credential_binding: Some(binding.clone()),
                    streamer_url: FakeProvider::URL.into(),
                    streams: cached,
                    media_headers: None,
                    media_extras: None,
                })),
            })
            .await;
        wait_for_start(&fixture).await;
        // The sidecar supplies identity and ordering, never the old URL.
        assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
        let config = fixture.engine.handles.lock()[0].config_snapshot();
        assert!(config.url.contains("fresh"), "{}", config.url);
        assert_eq!(config.session_id, session);
        assert_eq!(config.cookies.as_deref(), Some("session=selected"));
        let current = fixture
            .coordinator
            .stream_monitor
            .session_credential_binding(&session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current, binding, "the same account keeps its binding epoch");
        fixture.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn real_mesio_http_auth_responses_reach_one_diagnostic_before_session_finalization() {
    use crate::downloader::{DownloadFailureKind, DownloadTerminalEvent};
    use axum::{
        Router,
        http::{StatusCode, Uri},
        response::IntoResponse,
    };

    tokio::time::timeout(Duration::from_secs(30), async {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            for extension in ["flv", "m3u8", "master.m3u8", "refresh.m3u8"] {
                let mut fixture = Fixture::new().await;
                sqlx::query("UPDATE platform_config SET download_retry_policy = ? WHERE id = 'platform-twitch'")
                    .bind(r#"{"max_retries":1,"initial_delay_ms":0,"max_delay_ms":0,"use_jitter":false}"#)
                    .execute(&fixture.pool).await.unwrap();
                let (session, binding, repository) = managed(&fixture).await;
                let profile_id = bound_profile(&binding);
                let profile = repository.get(profile_id).await.unwrap();
                let diagnostic = Arc::new(RepeatingCheck {
                    calls: AtomicUsize::new(0),
                    status: bound_live(&binding, &profile, "renewed"),
                });
                Arc::get_mut(&mut fixture.coordinator).unwrap().recovery_check = Some(diagnostic.clone());
                let weak = Arc::downgrade(&fixture.coordinator);
                fixture.coordinator.download_manager.set_credential_diagnostic(Arc::new(move |event| {
                    let weak = weak.clone();
                    Box::pin(async move { weak.upgrade()?.diagnose_credential_attempt(event).await })
                }));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let requests = Arc::new(AtomicUsize::new(0));
                let counted = requests.clone();
                let server = AbortOnDropHandle::new(tokio::spawn(async move {
                    axum::serve(listener, Router::new().fallback(move |uri: Uri| {
                        let request = counted.fetch_add(1, Ordering::SeqCst);
                        async move {
                            if uri.path().ends_with("master.m3u8") {
                                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nchild.m3u8\n".into_response()
                            } else if uri.path().ends_with("refresh.m3u8") && request == 0 {
                                "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n".into_response()
                            } else { status.into_response() }
                        }
                    })).await.unwrap();
                }));
                let mut hls_config = mesio::HlsProtocolBuilder::new().get_config();
                hls_config.playlist_config.live_max_refresh_retries = 0;
                hls_config.playlist_config.live_refresh_interval = Duration::from_millis(10);
                fixture.coordinator.download_manager.register_engine(Arc::new(crate::downloader::engine::MesioEngine::new().with_hls_config(hls_config)));
                let mut events = fixture.coordinator.download_manager.subscribe();
                let mut config = DownloadConfig::new(
                    format!("http://{address}/live.{extension}"), fixture.directory.path(), STREAMER, "Coordinator", &session,
                );
                config.managed_credentials = true;
                config.credential_binding = Some(binding.clone());
                config.cookies = Some(profile.cookies.clone());
                fixture.coordinator.download_manager.start_download(config, Some("mesio".into()), false).await.unwrap();
                let terminal = loop {
                    if let DownloadManagerEvent::Terminal(terminal) = events.recv().await.unwrap() { break terminal; }
                };
                assert!(matches!(terminal, DownloadTerminalEvent::Failed {
                    kind: DownloadFailureKind::CredentialRecovery, recoverable: true, ..
                }), "{status} {extension}: {terminal:?}");
                assert!(requests.load(Ordering::SeqCst) > 0);
                assert_eq!(diagnostic.calls.load(Ordering::SeqCst), 1);
                assert!(repository.health(profile_id).await.unwrap().is_none_or(|health| health.validity != crate::credentials::CredentialValidity::Invalid));
                assert!(fixture.coordinator.session_lifecycle.is_session_active(&session));
                assert_eq!(fixture.coordinator.download_manager.active_count(), 0);
                // Startup consumes the full diagnostic bundle after the failed
                // engine settled, keeping both account and logical session.
                let mut handoff = payload(&session);
                handoff.credential_binding = Some(binding);
                run_live_download_pipeline(fixture.coordinator.clone(), handoff, false).await;
                fixture.engine.started.notified().await;
                let replacement = fixture.engine.handles.lock()[0].config_snapshot();
                assert!(replacement.url.contains("renewed"));
                assert_eq!(replacement.session_id, session);
                assert_eq!(replacement.cookies.as_deref(), Some("session=selected"));
                // Once the configured budget is spent, the next engine failure
                // retains its original classification and cannot loop forever.
                let exhausted = DownloadTerminalEvent::Failed {
                    download_id: "next-attempt".into(), streamer_id: STREAMER.into(),
                    streamer_name: "Coordinator".into(), session_id: session,
                    engine_type: EngineType::Mesio, protocol: crate::downloader::DownloadProtocol::Flv,
                    kind: DownloadFailureKind::HttpClientError { status: status.as_u16() },
                    error: "HTTP failure".into(), recoverable: false,
                };
                assert_eq!(fixture.coordinator.diagnose_credential_attempt(exhausted).await, None);
                assert_eq!(diagnostic.calls.load(Ordering::SeqCst), 1);
                fixture.close().await;
                drop(server);
            }
        }
    }).await.unwrap();
}

#[tokio::test]
async fn queued_managed_start_replaces_media_after_login_changes_the_pinned_account() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut fixture = Fixture::new().await;
        let (session, binding, repository) = managed(&fixture).await;
        let profile_id = bound_profile(&binding);
        let checker = FreshnessGate::new(Err(crate::Error::Monitor("not released".into())));
        fixture.freshness(checker.clone());
        let blocker = fixture.blocker().await;
        let coordinator = fixture.coordinator.clone();
        let mut handoff = payload(&session);
        handoff.credential_binding = Some(binding.clone());
        let pipeline = AbortOnDropHandle::new(tokio::spawn(async move {
            run_live_download_pipeline(coordinator, handoff, false).await;
        }));
        fixture.queued().await;
        // The download slot is still unavailable when new account material is
        // saved. Crossing the configured freshness threshold represents a long
        // queue wait without slowing this test down with wall-clock sleeps.
        let updated = repository
            .update(
                profile_id,
                1,
                None,
                None,
                Some(&CredentialMaterial {
                    cookies: "session=queue-login".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                }),
                None,
            )
            .await
            .unwrap();
        let mut fresh_binding = binding.clone();
        fresh_binding.revision = updated.revision as u64;
        *checker.result.lock() = Some(Ok(bound_live(&fresh_binding, &updated, "after-login")));
        checker.release.add_permits(1);
        drop(blocker);
        pipeline.await.unwrap();
        fixture.engine.started.notified().await;
        assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
        let config = fixture.engine.handles.lock()[0].config_snapshot();
        assert_eq!(config.cookies.as_deref(), Some("session=queue-login"));
        assert!(config.url.contains("after-login"));
        assert_eq!(config.session_id, session);
        let current = fixture
            .coordinator
            .stream_monitor
            .session_credential_binding(&session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.identity, binding.identity);
        assert_eq!(current.revision, 2);
        assert_eq!(current.epoch, binding.epoch + 1);
        fixture.close().await;
    })
    .await
    .unwrap();
}
