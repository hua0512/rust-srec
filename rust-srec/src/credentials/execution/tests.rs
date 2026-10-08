use super::*;
use crate::credentials::{CredentialOwner, CredentialScope};

/// The operation route of tests that do not exercise routes.
fn direct() -> &'static ResolvedRoute {
    static DIRECT: std::sync::LazyLock<ResolvedRoute> =
        std::sync::LazyLock::new(ResolvedRoute::default);
    &DIRECT
}

async fn fixture() -> (
    Arc<CredentialProfileRepository>,
    CredentialExecutionService,
    ResolvedCredentialPolicy,
    Vec<CredentialProfile>,
) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let repository = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
    let refresh = Arc::new(
        CredentialProviderRegistry::new()
            .with_admission(crate::credentials::test_support::unthrottled_admission()),
    );
    let service = CredentialExecutionService::new(repository.clone(), refresh);
    let owner = CredentialOwner::Platform {
        platform_id: "platform-huya".to_string(),
    };
    let mut profiles = Vec::new();
    for label in ["A", "B", "C"] {
        profiles.push(
            repository
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
    let policy = ResolvedCredentialPolicy::new(
        owner.id().to_string(),
        owner,
        CredentialSelection::Pool {
            credential_ids: profiles.iter().map(|profile| profile.id.clone()).collect(),
            strategy: PoolStrategy::RoundRobin,
            failover: true,
            max_attempts: 3,
        },
    )
    .unwrap();
    (repository, service, policy, profiles)
}

async fn selected(
    service: &CredentialExecutionService,
    policy: &ResolvedCredentialPolicy,
    binding: Option<&CredentialBinding>,
) -> CredentialExecution<String> {
    service
        .execute(
            policy,
            binding,
            false,
            OperationDeadline::new(Duration::from_secs(2)),
            direct(),
            |snapshot| async move {
                Ok(Extracted {
                    preserve_health: false,
                    value: snapshot.material.cookies,
                    session_cookies: None,
                })
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn unrecoverable_profile_authentication_emits_one_identified_invalid_notification() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (repository, service, policy, profiles) = fixture().await;
        let notifications = Arc::new(crate::notification::NotificationService::new());
        let mut received = notifications.subscribe();
        service
            .providers
            .set_notification_service(notifications.clone());
        let result = service
            .execute(
                &policy,
                None,
                true,
                OperationDeadline::default(), direct(),
                |snapshot| async move {
                    if snapshot.material.cookies == "account=A" {
                        Err(Error::Extractor(ExtractorError::Authentication {
                            code: "expired".into(),
                        }))
                    } else {
                        Ok(Extracted {
                            preserve_health: false,
                            value: snapshot.material.cookies,
                            session_cookies: None,
                        })
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(result.value, "account=B");
        assert_eq!(
            repository
                .health(&profiles[0].id)
                .await
                .unwrap()
                .unwrap()
                .validity,
            CredentialValidity::Invalid
        );
        let event = received.recv().await.unwrap();
        match event {
            crate::notification::NotificationEvent::Credential {
                event:
                    super::super::CredentialEvent::Invalid {
                        profile_id,
                        profile_label,
                        scope,
                        reason,
                        ..
                    },
            } => {
                assert_eq!(profile_id.as_deref(), Some(profiles[0].id.as_str()));
                assert_eq!(profile_label.as_deref(), Some("A"));
                assert!(matches!(
                    scope,
                    CredentialScope::Platform { platform_id, .. } if platform_id == policy.platform_id
                ));
                assert_eq!(reason, "login_required");
            }
            event => panic!("unexpected notification: {event:?}"),
        }
        let fixed = ResolvedCredentialPolicy::new(
            policy.platform_id,
            policy.owner,
            CredentialSelection::Fixed {
                credential_id: profiles[0].id.clone(),
            },
        )
        .unwrap();
        assert!(
            service
                .execute(
                    &fixed,
                    None,
                    true,
                    OperationDeadline::default(), direct(),
                    |_| async {
                        Ok(Extracted {
                            preserve_health: false,
                            value: (),
                            session_cookies: None,
                        })
                    }
                )
                .await
                .is_err()
        );
        assert!(matches!(
            received.recv().await.unwrap(),
            crate::notification::NotificationEvent::Credential {
                event: super::super::CredentialEvent::Unavailable { .. }
            }
        ));
        notifications.stop().await;
        assert!(
            received.try_recv().is_err(),
            "cached invalid account must not emit duplicate profile notifications"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn ordinary_offline_ends_operation_and_unknown_throttle_keeps_account_healthy() {
    let (repository, service, policy, profiles) = fixture().await;
    let offline = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |_| async {
                Ok(Extracted {
                    preserve_health: true,
                    value: "offline",
                    session_cookies: None,
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(offline.value, "offline");
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let error = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async {
                    Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::RateLimited {
                        code: None,
                        retry_after: Some(Duration::from_millis(1)),
                    }))
                }
            },
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::Extractor(ExtractorError::RateLimited { .. })
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(repository.health(&profiles[1].id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_throttled_account_hands_over_to_one_on_another_route() {
    let (repository, service, policy, profiles) = fixture().await;
    let pool = repository.write_pool_for_tests();
    let shared = crate::database::repositories::proxies::save_for_test(
        &pool,
        "shared",
        "http://shared.example:8080",
    )
    .await;
    let own = crate::database::repositories::proxies::save_for_test(
        &pool,
        "own",
        "http://own.example:8080",
    )
    .await;
    // A and B leave through the same proxy, C through its own.
    for (profile, id) in profiles.iter().zip([&shared, &shared, &own]) {
        repository
            .update(
                &profile.id,
                profile.version,
                None,
                None,
                None,
                Some(&crate::proxies::ProxyRoute::Proxy { id: id.clone() }),
            )
            .await
            .unwrap();
    }
    let policy = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Pool {
            credential_ids: profiles.iter().map(|profile| profile.id.clone()).collect(),
            strategy: PoolStrategy::Priority,
            failover: true,
            max_attempts: 3,
        },
    )
    .unwrap();
    let used = Arc::new(Mutex::new(Vec::new()));
    let operation = ResolvedRoute::default();
    let run = || {
        let used = used.clone();
        service.execute(
            &policy,
            None,
            true,
            OperationDeadline::new(Duration::from_secs(5)),
            &operation,
            move |snapshot| {
                used.lock().push((
                    snapshot.material.cookies.clone(),
                    snapshot.route.proxy_name().map(str::to_owned),
                    snapshot
                        .route
                        .target
                        .endpoint()
                        .map(|endpoint| endpoint.url.clone()),
                ));
                async move {
                    if snapshot.material.cookies == "account=A" {
                        return Err(Error::Extractor(ExtractorError::RateLimited {
                            code: None,
                            retry_after: Some(Duration::from_secs(60)),
                        }));
                    }
                    Ok(Extracted {
                        preserve_health: false,
                        value: snapshot.material.cookies,
                        session_cookies: None,
                    })
                }
            },
        )
    };
    let execution = run().await.unwrap();
    assert_eq!(execution.value, "account=C");
    // The snapshot carries the route, so the download takes the same exit.
    assert_eq!(execution.snapshot.route.proxy_name(), Some("own"));
    assert_eq!(
        *used.lock(),
        [
            (
                "account=A".to_owned(),
                Some("shared".to_owned()),
                Some("http://shared.example:8080".to_owned())
            ),
            (
                "account=C".to_owned(),
                Some("own".to_owned()),
                Some("http://own.example:8080".to_owned())
            ),
        ],
        "B shares A's throttled proxy, so it is passed over"
    );
    let admission = service.providers.admission();
    assert!(admission.backing_off(&policy.platform_id, &RouteKey::Proxy { id: shared }));
    assert!(!admission.backing_off(&policy.platform_id, &RouteKey::Direct));
    assert!(repository.health(&profiles[0].id).await.unwrap().is_none());

    // While A's route is paused, the next operation starts on an account that
    // can answer now.
    used.lock().clear();
    assert_eq!(run().await.unwrap().value, "account=C");
    assert_eq!(used.lock().len(), 1);
}

#[tokio::test]
async fn accounts_without_their_own_route_follow_the_operation() {
    let (repository, service, policy, profiles) = fixture().await;
    let pool = repository.write_pool_for_tests();
    let recording = crate::database::repositories::proxies::save_for_test(
        &pool,
        "recording",
        "socks5h://recording.example:1080",
    )
    .await;
    let operation = crate::proxies::materialize(
        &crate::proxies::ProxyRoute::Proxy {
            id: recording.clone(),
        },
        crate::proxies::RouteSource::Streamer,
        Some(
            &crate::database::repositories::proxies::find(
                &mut pool.acquire().await.unwrap(),
                &recording,
            )
            .await
            .unwrap()
            .unwrap(),
        ),
        &crate::proxies::SystemProxy::default(),
    )
    .unwrap()
    .unwrap();
    let policy = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    let execution = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::new(Duration::from_secs(5)),
            &operation,
            |snapshot| async move {
                Ok(Extracted {
                    preserve_health: false,
                    value: snapshot.route.clone(),
                    session_cookies: None,
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(execution.value, operation);
    assert_eq!(execution.snapshot.route, operation);
}

#[tokio::test]
async fn disabled_profiles_are_skipped_even_when_failover_is_off() {
    let (repository, service, policy, profiles) = fixture().await;
    repository
        .update(&profiles[0].id, 1, None, Some(false), None, None)
        .await
        .unwrap();
    let policy = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Pool {
            credential_ids: profiles.iter().map(|profile| profile.id.clone()).collect(),
            strategy: PoolStrategy::Priority,
            failover: false,
            max_attempts: 3,
        },
    )
    .unwrap();
    assert_eq!(selected(&service, &policy, None).await.value, "account=B");
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let error = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async {
                    Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::Authentication {
                        code: "expired".into(),
                    }))
                }
            },
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::CredentialUnavailable(_)));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profiles[1].id.clone(),
        },
        revision: 1,
        policy: policy.clone(),
        epoch: 1,
    };
    assert!(matches!(
        service
            .execute(
                &policy,
                Some(&binding),
                true,
                OperationDeadline::default(),
                direct(),
                |_| async {
                    Err::<Extracted<()>, _>(Error::Other("unexpected alternate account".into()))
                }
            )
            .await,
        Err(Error::CredentialUnavailable(_))
    ));
}

#[tokio::test]
async fn ineligible_bound_account_recovers_to_its_next_saved_candidate_without_advancing_cursor() {
    let (repository, service, policy, profiles) = fixture().await;
    let bound = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profiles[1].id.clone(),
        },
        revision: 1,
        policy: policy.clone(),
        epoch: 4,
    };
    repository
        .publish_health(
            &profiles[1],
            CredentialValidity::Invalid,
            Some(HealthReason::LoginRequired),
            false,
        )
        .await
        .unwrap();
    let result = service
        .execute(
            &policy,
            Some(&bound),
            true,
            OperationDeadline::default(),
            direct(),
            |snapshot| async move {
                Ok(Extracted {
                    preserve_health: false,
                    value: snapshot.material.cookies,
                    session_cookies: None,
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(result.value, "account=C");
    assert_eq!(selected(&service, &policy, None).await.value, "account=A");
}

#[tokio::test]
async fn concurrent_independent_operations_reserve_rotation_once_each() {
    let (_repository, service, policy, _) = fixture().await;
    let results = tokio::time::timeout(
        Duration::from_secs(5),
        futures::future::join_all((0..30).map(|_| selected(&service, &policy, None))),
    )
    .await
    .unwrap();
    for account in ["account=A", "account=B", "account=C"] {
        assert_eq!(
            results
                .iter()
                .filter(|result| result.value == account)
                .count(),
            10
        );
    }
}

#[tokio::test]
async fn none_suppresses_stored_authentication_but_keeps_generated_guest_cookies_ephemeral() {
    let (repository, service, policy, profiles) = fixture().await;
    let policy =
        ResolvedCredentialPolicy::new(policy.platform_id, policy.owner, CredentialSelection::None)
            .unwrap();
    let result = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |snapshot| async move {
                assert!(snapshot.material.cookies.is_empty());
                assert!(snapshot.material.reauth_config.is_none());
                Ok(Extracted {
                    preserve_health: false,
                    value: (),
                    session_cookies: Some("guest=device".into()),
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(result.snapshot.material.cookies, "guest=device");
    assert!(matches!(
        result.snapshot.binding.identity,
        CredentialIdentity::Anonymous
    ));
    assert_eq!(repository.get(&profiles[0].id).await.unwrap().revision, 1);
}

#[tokio::test]
async fn unavailable_status_survives_reading_and_clears_after_successful_replacement() {
    let (repository, service, policy, profiles) = fixture().await;
    let fixed = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    repository
        .update(&profiles[0].id, 1, None, Some(false), None, None)
        .await
        .unwrap();
    assert!(
        service
            .execute(
                &fixed,
                None,
                false,
                OperationDeadline::default(),
                direct(),
                |_| async {
                    Ok(Extracted {
                        preserve_health: false,
                        value: (),
                        session_cookies: None,
                    })
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        service.unavailable_status(&fixed).unwrap().reason,
        UnavailableReason::ProfilesDisabled
    );
    assert_eq!(
        service.unavailable_status(&fixed).unwrap().reason,
        UnavailableReason::ProfilesDisabled
    );
    repository
        .update(&profiles[0].id, 2, None, Some(true), None, None)
        .await
        .unwrap();
    selected(&service, &fixed, None).await;
    assert!(service.unavailable_status(&fixed).is_none());
    assert_eq!(
        repository
            .health(&profiles[0].id)
            .await
            .unwrap()
            .unwrap()
            .validity,
        CredentialValidity::Unknown,
        "unsupported validation must not invent a validated conclusion"
    );
}

struct RecordingRefresh {
    inputs: Arc<Mutex<Vec<CredentialMaterial>>>,
}

#[async_trait::async_trait]
impl super::super::CredentialProvider for RecordingRefresh {
    fn capabilities(&self) -> super::super::ProviderCapabilities {
        super::super::ProviderCapabilities {
            check: true,
            refresh: true,
            ..Default::default()
        }
    }
    async fn check(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> std::result::Result<AccountStatus, CredentialError> {
        Ok(AccountStatus::Valid)
    }
    async fn refresh(
        &self,
        _client: &reqwest::Client,
        state: &CredentialMaterial,
    ) -> std::result::Result<RefreshedCredentials, CredentialError> {
        self.inputs.lock().push(state.clone());
        Ok(RefreshedCredentials {
            cookies: "account=repaired".into(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        })
    }
}

#[tokio::test]
async fn repair_uses_bound_access_token_and_consumes_another_attempt() {
    let (repository, mut service, policy, profiles) = fixture().await;
    let inputs = Arc::new(Mutex::new(Vec::new()));
    Arc::get_mut(&mut service.providers)
        .unwrap()
        .register_provider(
            "huya",
            Arc::new(RecordingRefresh {
                inputs: inputs.clone(),
            }),
        );
    repository
        .update(
            &profiles[0].id,
            1,
            None,
            None,
            Some(&CredentialMaterial {
                cookies: "account=A".into(),
                refresh_token: Some("refresh-a".into()),
                access_token: Some("access-a".into()),
                reauth_config: None,
            }),
            None,
        )
        .await
        .unwrap();
    let fixed = ResolvedCredentialPolicy::new(
        policy.platform_id.clone(),
        policy.owner.clone(),
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let result = service
        .execute(
            &fixed,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |snapshot| {
                let attempt = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async move {
                    if attempt == 0 {
                        Err(Error::Extractor(ExtractorError::Authentication {
                            code: "expired".into(),
                        }))
                    } else {
                        Ok(Extracted {
                            preserve_health: false,
                            value: snapshot.material.cookies,
                            session_cookies: None,
                        })
                    }
                }
            },
        )
        .await
        .unwrap();
    assert_eq!(result.value, "account=repaired");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(inputs.lock().len(), 1);
    assert_eq!(inputs.lock()[0].access_token.as_deref(), Some("access-a"));
    assert_eq!(
        repository
            .get(&profiles[0].id)
            .await
            .unwrap()
            .access_token
            .as_deref(),
        Some("access-a")
    );
    for untouched in &profiles[1..] {
        let current = repository.get(&untouched.id).await.unwrap();
        assert_eq!(current.cookies, untouched.cookies);
        assert_eq!(current.revision, untouched.revision);
        assert_eq!(current.version, untouched.version);
        assert_eq!(current.refresh_token, untouched.refresh_token);
        assert_eq!(current.access_token, untouched.access_token);
        assert_eq!(current.reauth_config, untouched.reauth_config);
        assert!(repository.health(&untouched.id).await.unwrap().is_none());
    }
    let single = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Pool {
            credential_ids: vec![profiles[1].id.clone()],
            strategy: PoolStrategy::Priority,
            failover: true,
            max_attempts: 1,
        },
    )
    .unwrap();
    let error = service
        .execute(
            &single,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |_| async {
                Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::Authentication {
                    code: "expired".into(),
                }))
            },
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::CredentialUnavailable(_)));
    assert_eq!(
        inputs.lock().len(),
        1,
        "repair must not escape a one-attempt budget"
    );
}

struct GatedRefresh {
    started: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl super::super::CredentialProvider for GatedRefresh {
    fn capabilities(&self) -> super::super::ProviderCapabilities {
        super::super::ProviderCapabilities {
            check: true,
            refresh: true,
            ..Default::default()
        }
    }
    async fn check(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> std::result::Result<AccountStatus, CredentialError> {
        Ok(AccountStatus::Repairable)
    }
    async fn refresh(
        &self,
        _client: &reqwest::Client,
        _state: &CredentialMaterial,
    ) -> std::result::Result<RefreshedCredentials, CredentialError> {
        self.started.notify_one();
        self.release.acquire().await.unwrap().forget();
        Ok(RefreshedCredentials {
            cookies: "account=late-provider".into(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        })
    }
}

#[tokio::test]
async fn profile_refresh_allows_label_edits_but_rejects_replaced_account_material() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for replacement in [false, true] {
            let (repository, mut service, _policy, profiles) = fixture().await;
            let manager = Arc::new(GatedRefresh { started: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0) });
            Arc::get_mut(&mut service.providers).unwrap().register_provider("huya", manager.clone());
            let operation = service.refresh_profile(&profiles[0].id, OperationDeadline::default()); tokio::pin!(operation);
            tokio::select! { _ = &mut operation => panic!("provider must wait"), _ = manager.started.notified() => {} }
            let replacement_material = CredentialMaterial { cookies: "account=user-replaced".into(), refresh_token: None, access_token: None, reauth_config: None };
            repository.update(&profiles[0].id, 1, Some("Renamed"), None, replacement.then_some(&replacement_material), None).await.unwrap();
            manager.release.add_permits(1);
            let outcome = operation.await;
            if replacement { assert!(matches!(outcome, Err(Error::CredentialProfile(ProfileError::SourceChanged)))); }
            else { assert!(outcome.is_ok()); }
            let current = repository.get(&profiles[0].id).await.unwrap();
            assert_eq!(current.label, "Renamed");
            assert_eq!(current.cookies, if replacement { "account=user-replaced" } else { "account=late-provider" });
        }
    }).await.unwrap();
}

#[tokio::test]
async fn concurrent_health_change_after_reservation_prevents_stale_account_extraction() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (repository, service, policy, profiles) = fixture().await;
        service.providers.admission().defer(
            &policy.platform_id,
            &ResolvedRoute::default(),
            Duration::from_millis(500),
        );
        let used = Arc::new(Mutex::new(Vec::new()));
        let captured = used.clone();
        let operation = service.execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            move |snapshot| {
                let captured = captured.clone();
                async move {
                    captured.lock().push(snapshot.material.cookies.clone());
                    Ok(Extracted {
                        preserve_health: false,
                        value: snapshot.material.cookies,
                        session_cookies: None,
                    })
                }
            },
        );
        tokio::pin!(operation);
        // Keep driving the operation until it parks on the platform backoff.
        // The in-memory fixture has one connection, so stopping mid-query
        // would leave the health write below waiting for that connection.
        tokio::select! {
            _ = &mut operation => panic!("platform backoff must hold extraction"),
            () = tokio::time::sleep(Duration::from_millis(200)) => {},
        }
        assert!(service.cursors.lock().contains_key(&policy.generation));
        repository
            .publish_health(
                &profiles[0],
                CredentialValidity::Invalid,
                Some(HealthReason::LoginRequired),
                false,
            )
            .await
            .unwrap();
        let result = operation.await.unwrap();
        assert_eq!(result.value, "account=B");
        assert_eq!(*used.lock(), vec!["account=B"]);
    })
    .await
    .unwrap();
}

#[test]
fn engine_diagnostic_gate_does_not_promote_generic_http_or_local_failures() {
    use crate::downloader::DownloadFailureKind as Kind;
    use crate::downloader::engine::EngineType;
    assert!(
        Kind::HttpClientError { status: 401 }.requests_credential_diagnostic(EngineType::Mesio)
    );
    assert!(
        Kind::HttpClientError { status: 403 }.requests_credential_diagnostic(EngineType::Mesio)
    );
    assert!(!Kind::HttpClientError { status: 403 }.is_recoverable());
    assert!(Kind::ProcessExit { code: Some(1) }.requests_credential_diagnostic(EngineType::Ffmpeg));
    for kind in [
        Kind::Io,
        Kind::Processing,
        Kind::Configuration,
        Kind::Cancelled,
        Kind::HttpClientError { status: 404 },
        Kind::SourceUnavailable,
    ] {
        for engine in [
            EngineType::Mesio,
            EngineType::Ffmpeg,
            EngineType::Streamlink,
        ] {
            assert!(!kind.requests_credential_diagnostic(engine));
        }
    }
}

/// A provider that found nothing to refresh returns the stored bundle.
struct UnchangedRefresh {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl super::super::CredentialProvider for UnchangedRefresh {
    fn capabilities(&self) -> super::super::ProviderCapabilities {
        super::super::ProviderCapabilities {
            check: true,
            refresh: true,
            ..Default::default()
        }
    }
    async fn check(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> std::result::Result<AccountStatus, CredentialError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(AccountStatus::Valid)
    }
    async fn refresh(
        &self,
        _client: &reqwest::Client,
        state: &CredentialMaterial,
    ) -> std::result::Result<RefreshedCredentials, CredentialError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(RefreshedCredentials {
            cookies: state.cookies.clone(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        })
    }
}

#[tokio::test]
async fn unchanged_refresh_keeps_the_revision_bound_recordings_rely_on() {
    let (repository, mut service, _policy, profiles) = fixture().await;
    Arc::get_mut(&mut service.providers)
        .unwrap()
        .register_provider(
            "huya",
            Arc::new(UnchangedRefresh {
                calls: Default::default(),
            }),
        );
    let profile = &profiles[0];
    repository
        .publish_health(
            profile,
            CredentialValidity::NeedsRefresh,
            Some(HealthReason::AuthenticationFailed),
            false,
        )
        .await
        .unwrap();
    let result = service
        .refresh_profile(&profile.id, OperationDeadline::new(Duration::from_secs(5)))
        .await
        .unwrap();
    assert!(result.supported);
    let current = repository.get(&profile.id).await.unwrap();
    assert_eq!(current.revision, profile.revision);
    assert_eq!(current.version, profile.version);
    let health = repository.health(&profile.id).await.unwrap().unwrap();
    assert_eq!(health.validity, CredentialValidity::Valid);
    assert!(health.last_refresh_at.is_some());
    assert_eq!(health.reason_code, None);
}

#[tokio::test]
async fn disabled_profiles_reject_manual_actions_before_any_provider_call() {
    let (repository, mut service, _policy, profiles) = fixture().await;
    let manager = Arc::new(UnchangedRefresh {
        calls: Default::default(),
    });
    Arc::get_mut(&mut service.providers)
        .unwrap()
        .register_provider("huya", manager.clone());
    repository
        .update(&profiles[0].id, 1, None, Some(false), None, None)
        .await
        .unwrap();
    let deadline = OperationDeadline::new(Duration::from_secs(5));
    assert!(matches!(
        service.validate_profile(&profiles[0].id, deadline).await,
        Err(Error::CredentialProfile(ProfileError::Disabled))
    ));
    assert!(matches!(
        service.refresh_profile(&profiles[0].id, deadline).await,
        Err(Error::CredentialProfile(ProfileError::Disabled))
    ));
    assert_eq!(manager.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn anonymous_platform_throttle_delays_every_caller_of_the_platform() {
    let (_repository, service, policy, _) = fixture().await;
    let anonymous =
        ResolvedCredentialPolicy::new(policy.platform_id, policy.owner, CredentialSelection::None)
            .unwrap();
    let error = service
        .execute(
            &anonymous,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |_| async {
                Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::RateLimited {
                    code: None,
                    retry_after: Some(Duration::from_secs(30)),
                }))
            },
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::Extractor(ExtractorError::RateLimited { .. })
    ));
    assert!(
        service
            .providers
            .admission()
            .admit(
                &anonymous.platform_id,
                &RouteKey::Direct,
                OperationDeadline::new(Duration::from_millis(200))
            )
            .await
            .is_err(),
        "managed and anonymous callers share the platform backoff"
    );
}

#[tokio::test]
async fn exhaustion_names_the_action_that_would_recover_the_selection() {
    let (repository, service, policy, profiles) = fixture().await;
    let reason = |service: &CredentialExecutionService| {
        service
            .unavailable_status(&policy)
            .map(|status| status.reason)
    };
    let attempt = || {
        service.execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |_| async { Err::<Extracted<()>, _>(Error::Other("no candidate may be tried".into())) },
        )
    };
    repository
        .publish_health(
            &profiles[0],
            CredentialValidity::Invalid,
            Some(HealthReason::LoginRequired),
            false,
        )
        .await
        .unwrap();
    repository
        .update(&profiles[1].id, 1, None, Some(false), None, None)
        .await
        .unwrap();
    repository
        .publish_health(
            &profiles[2],
            CredentialValidity::Invalid,
            Some(HealthReason::LoginRequired),
            false,
        )
        .await
        .unwrap();
    assert!(attempt().await.is_err());
    assert_eq!(reason(&service), Some(UnavailableReason::LoginRequired));

    for profile in [&profiles[0], &profiles[2]] {
        let current = repository.get(&profile.id).await.unwrap();
        repository
            .update(&profile.id, current.version, None, Some(false), None, None)
            .await
            .unwrap();
    }
    assert!(attempt().await.is_err());
    assert_eq!(reason(&service), Some(UnavailableReason::ProfilesDisabled));
}

/// The acceptance storyline at the execution boundary: A/B/C rotate
/// across independent checks, a recording pinned to B keeps B on its polls,
/// B expiring recovers the recording with C through a fresh extraction, a
/// targeted refresh of A leaves B and C untouched, and a streamer fixed to A
/// never uses another account.
#[tokio::test]
async fn acceptance_rotation_pinning_recovery_targeted_refresh_and_fixed_isolation() {
    let (repository, mut service, pool, profiles) = fixture().await;
    let identity = |snapshot: &CredentialSnapshot| match &snapshot.binding.identity {
        CredentialIdentity::Profile { profile_id } => profile_id.clone(),
        other => panic!("unexpected identity {other:?}"),
    };

    // Independent checks rotate through the pool.
    for account in ["account=A", "account=B", "account=C"] {
        assert_eq!(selected(&service, &pool, None).await.value, account);
    }

    // A recording pinned to B polls B and does not advance rotation.
    let pinned = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profiles[1].id.clone(),
        },
        revision: 1,
        policy: pool.clone(),
        epoch: 1,
    };
    for _ in 0..2 {
        assert_eq!(
            selected(&service, &pool, Some(&pinned)).await.value,
            "account=B"
        );
    }
    assert_eq!(selected(&service, &pool, None).await.value, "account=A");

    // B expires: the pinned poll may not switch accounts...
    let expired_b = |snapshot: CredentialSnapshot| {
        let b = snapshot.material.cookies == "account=B";
        async move {
            if b {
                Err(Error::Extractor(ExtractorError::Authentication {
                    code: "expired".into(),
                }))
            } else {
                Ok(Extracted {
                    preserve_health: false,
                    value: snapshot.material.cookies,
                    session_cookies: None,
                })
            }
        }
    };
    let polled = service
        .execute(
            &pool,
            Some(&pinned),
            false,
            OperationDeadline::default(),
            direct(),
            expired_b,
        )
        .await;
    assert!(matches!(polled, Err(Error::CredentialUnavailable(_))));
    // ...so recovery extracts fresh media with the next saved account, C.
    let recovered = service
        .execute(
            &pool,
            Some(&pinned),
            true,
            OperationDeadline::default(),
            direct(),
            expired_b,
        )
        .await
        .unwrap();
    assert_eq!(recovered.value, "account=C");
    assert_eq!(identity(&recovered.snapshot), profiles[2].id);
    assert_eq!(recovered.snapshot.binding.epoch, pinned.epoch);

    // A targeted refresh of A changes only A.
    let inputs = Arc::new(Mutex::new(Vec::new()));
    Arc::get_mut(&mut service.providers)
        .unwrap()
        .register_provider(
            "huya",
            Arc::new(RecordingRefresh {
                inputs: inputs.clone(),
            }),
        );
    let before: Vec<_> = futures::future::join_all(
        profiles[1..]
            .iter()
            .map(|profile| repository.get(&profile.id)),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();
    let refreshed = service
        .refresh_profile(&profiles[0].id, OperationDeadline::default())
        .await
        .unwrap();
    assert_eq!(refreshed.profile.id, profiles[0].id);
    assert_eq!(inputs.lock().len(), 1);
    assert_eq!(inputs.lock()[0].cookies, "account=A");
    for previous in before {
        let current = repository.get(&previous.id).await.unwrap();
        assert_eq!(
            (current.cookies, current.revision, current.version),
            (previous.cookies, previous.revision, previous.version)
        );
    }

    // A streamer fixed to A stays on A even when A fails.
    let fixed = ResolvedCredentialPolicy::new(
        pool.platform_id.clone(),
        pool.owner.clone(),
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    let seen = Mutex::new(Vec::new());
    let result = service
        .execute(
            &fixed,
            None,
            true,
            OperationDeadline::default(),
            direct(),
            |snapshot| {
                seen.lock().push(identity(&snapshot));
                async {
                    Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::Authentication {
                        code: "expired".into(),
                    }))
                }
            },
        )
        .await;
    assert!(matches!(result, Err(Error::CredentialUnavailable(_))));
    let seen = seen.into_inner();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|id| *id == profiles[0].id), "{seen:?}");
}

/// A provider that checks accounts but cannot refresh them (Twitch). The
/// account whose cookies are `revoked` is rejected; `None` marks every
/// account as having nothing to check.
struct CheckOnly {
    revoked: Option<&'static str>,
    checks: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl super::super::CredentialProvider for CheckOnly {
    fn capabilities(&self) -> super::super::ProviderCapabilities {
        super::super::ProviderCapabilities {
            check: true,
            ..Default::default()
        }
    }
    async fn check(
        &self,
        _client: &reqwest::Client,
        material: &CredentialMaterial,
    ) -> std::result::Result<AccountStatus, CredentialError> {
        self.checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(match self.revoked {
            None => AccountStatus::Unverifiable,
            Some(revoked) if revoked == material.cookies => AccountStatus::Revoked,
            Some(_) => AccountStatus::Valid,
        })
    }
}

async fn extract_cookies(
    service: &CredentialExecutionService,
    policy: &ResolvedCredentialPolicy,
) -> Result<String> {
    service
        .execute(
            policy,
            None,
            true,
            OperationDeadline::new(Duration::from_secs(5)),
            direct(),
            |snapshot| async move {
                Ok(Extracted {
                    preserve_health: false,
                    value: snapshot.material.cookies,
                    session_cookies: None,
                })
            },
        )
        .await
        .map(|execution| execution.value)
}

#[tokio::test]
async fn a_revoked_account_that_cannot_refresh_is_invalidated_before_extraction() {
    let (repository, mut service, policy, profiles) = fixture().await;
    let manager = Arc::new(CheckOnly {
        revoked: Some("account=A"),
        checks: Default::default(),
    });
    Arc::get_mut(&mut service.providers)
        .unwrap()
        .register_provider("huya", manager.clone());

    for _ in 0..3 {
        let cookies = extract_cookies(&service, &policy).await.unwrap();
        assert_ne!(cookies, "account=A");
    }

    let health = repository.health(&profiles[0].id).await.unwrap().unwrap();
    assert_eq!(health.validity, CredentialValidity::Invalid);
    let checked = repository.health(&profiles[1].id).await.unwrap().unwrap();
    assert_eq!(checked.validity, CredentialValidity::Valid);
    let result = service
        .validate_profile(
            &profiles[0].id,
            OperationDeadline::new(Duration::from_secs(5)),
        )
        .await
        .unwrap();
    assert_eq!(result.health.unwrap().validity, CredentialValidity::Invalid);
}

#[tokio::test]
async fn an_account_with_nothing_to_check_records_and_stays_unchecked() {
    let (repository, mut service, policy, profiles) = fixture().await;
    let manager = Arc::new(CheckOnly {
        revoked: None,
        checks: Default::default(),
    });
    Arc::get_mut(&mut service.providers)
        .unwrap()
        .register_provider("huya", manager.clone());

    for _ in 0..2 {
        extract_cookies(&service, &policy).await.unwrap();
    }

    assert!(manager.checks.load(std::sync::atomic::Ordering::Relaxed) >= 2);
    for profile in &profiles {
        if let Some(health) = repository.health(&profile.id).await.unwrap() {
            assert_eq!(health.validity, CredentialValidity::Unknown);
            assert!(health.last_check_at.is_none());
        }
    }
    assert!(matches!(
        service
            .validate_profile(
                &profiles[0].id,
                OperationDeadline::new(Duration::from_secs(5))
            )
            .await,
        Err(Error::CredentialProfile(ProfileError::InvalidMaterial(_)))
    ));
}

/// A provider whose sessions lapse silently, as Douyu's do: accounts with a
/// refresh token are renewed once their material is old enough.
#[derive(Default)]
struct Renewing {
    refreshes: std::sync::atomic::AtomicUsize,
    /// 0 renews, 1 fails without demanding a login, 2 demands a new login.
    outcome: std::sync::atomic::AtomicU8,
}

#[async_trait::async_trait]
impl super::super::CredentialProvider for Renewing {
    fn capabilities(&self) -> super::super::ProviderCapabilities {
        super::super::ProviderCapabilities {
            refresh_token: true,
            refresh: true,
            ..Default::default()
        }
    }
    fn refreshable(&self, material: &CredentialMaterial) -> bool {
        material.refresh_token.is_some()
    }
    fn renew_after(&self) -> Option<Duration> {
        Some(Duration::from_secs(4 * 24 * 60 * 60))
    }
    async fn refresh(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> std::result::Result<RefreshedCredentials, CredentialError> {
        let count = self
            .refreshes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.outcome.load(std::sync::atomic::Ordering::SeqCst) {
            0 => Ok(RefreshedCredentials {
                cookies: format!("account=renewed-{count}"),
                refresh_token: None,
                access_token: None,
                expires_at: None,
            }),
            1 => Err(CredentialError::RefreshFailed("provider error".into())),
            _ => Err(CredentialError::InvalidRefreshToken),
        }
    }
}

#[tokio::test]
async fn aging_material_is_renewed_before_use_and_a_failed_renewal_keeps_it() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repository = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
        let provider = Arc::new(Renewing::default());
        let mut refresh = CredentialProviderRegistry::new()
            .with_admission(crate::credentials::test_support::unthrottled_admission());
        refresh.register_provider("huya", provider.clone());
        let service = CredentialExecutionService::new(repository.clone(), Arc::new(refresh));
        let owner = CredentialOwner::Platform {
            platform_id: "platform-huya".to_string(),
        };
        let create = |label: &'static str, refresh_token: Option<&str>| {
            let repository = repository.clone();
            let owner = owner.clone();
            let refresh_token = refresh_token.map(str::to_owned);
            async move {
                repository
                    .create(
                        owner.id(),
                        label,
                        true,
                        &CredentialMaterial {
                            cookies: format!("account={label}"),
                            refresh_token,
                            access_token: None,
                            reauth_config: None,
                        },
                        &crate::proxies::ProxyRoute::Inherit,
                    )
                    .await
                    .unwrap()
            }
        };
        let renewable = create("A", Some("passport")).await;
        let cookies_only = create("B", None).await;
        let fixed = |profile: &CredentialProfile| {
            ResolvedCredentialPolicy::new(
                owner.id().to_string(),
                owner.clone(),
                CredentialSelection::Fixed {
                    credential_id: profile.id.clone(),
                },
            )
            .unwrap()
        };
        let days_ago = |days: i64| crate::database::time::now_ms() - days * 24 * 60 * 60 * 1000;
        let refreshes = || provider.refreshes.load(std::sync::atomic::Ordering::SeqCst);

        // Fresh material is used as it is.
        let policy = fixed(&renewable);
        assert_eq!(
            extract_cookies(&service, &policy).await.unwrap(),
            "account=A"
        );
        assert_eq!(refreshes(), 0);

        // Material older than the renewal age is renewed first, once.
        sqlx::query("UPDATE credential_profiles SET updated_at = ?")
            .bind(days_ago(5))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            extract_cookies(&service, &policy).await.unwrap(),
            "account=renewed-0"
        );
        assert_eq!(
            extract_cookies(&service, &policy).await.unwrap(),
            "account=renewed-0"
        );
        assert_eq!(refreshes(), 1);
        let health = repository.health(&renewable.id).await.unwrap().unwrap();
        assert_eq!(health.validity, CredentialValidity::Valid);
        assert!(health.last_refresh_at.is_some());

        // An account that cannot be renewed is used without a refresh, and a
        // manual refresh offers none.
        let cookies_policy = fixed(&cookies_only);
        assert_eq!(
            extract_cookies(&service, &cookies_policy).await.unwrap(),
            "account=B"
        );
        assert_eq!(refreshes(), 1);
        let manual = service
            .refresh_profile(
                &cookies_only.id,
                OperationDeadline::new(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        assert!(!manual.supported);
        assert!(
            manual
                .health
                .is_none_or(|health| health.validity == CredentialValidity::Unknown)
        );

        // The renewal age counts from the last refresh. A failed renewal keeps
        // the current material and is not retried within the retry window.
        sqlx::query("UPDATE credential_profile_health SET last_refresh_at = ?")
            .bind(days_ago(5))
            .execute(&pool)
            .await
            .unwrap();
        provider
            .outcome
            .store(1, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..2 {
            assert_eq!(
                extract_cookies(&service, &policy).await.unwrap(),
                "account=renewed-0"
            );
        }
        assert_eq!(refreshes(), 2);
        let health = repository.health(&renewable.id).await.unwrap().unwrap();
        assert_eq!(health.validity, CredentialValidity::NeedsRefresh);
        assert_eq!(health.refresh_failure_count, 1);

        // After the retry window, a renewal that demands a new login stops the
        // account.
        sqlx::query("UPDATE credential_profile_health SET last_failure_at = ?")
            .bind(crate::database::time::now_ms() - 2 * 60 * 60 * 1000)
            .execute(&pool)
            .await
            .unwrap();
        provider
            .outcome
            .store(2, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            extract_cookies(&service, &policy).await,
            Err(Error::CredentialUnavailable(_))
        ));
        assert_eq!(refreshes(), 3);
        assert_eq!(
            repository
                .health(&renewable.id)
                .await
                .unwrap()
                .unwrap()
                .validity,
            CredentialValidity::Invalid
        );
    })
    .await
    .expect("renewal test timed out");
}

fn profile_at(updated_at: i64, refresh_token: Option<&str>) -> CredentialProfile {
    CredentialProfile {
        id: "renewing".into(),
        platform_config_id: "platform-huya".into(),
        label: "Renewing".into(),
        enabled: true,
        cookies: "account=A".into(),
        refresh_token: refresh_token.map(str::to_owned),
        access_token: None,
        reauth_config: None,
        revision: 1,
        version: 1,
        created_at: updated_at,
        updated_at,
        last_used_at: None,
        proxy_route: "inherit".into(),
        proxy_id: None,
    }
}

fn health_with(
    validity: CredentialValidity,
    last_refresh_at: Option<i64>,
    last_failure_at: Option<i64>,
) -> CredentialProfileHealth {
    CredentialProfileHealth {
        profile_id: "renewing".into(),
        revision: 1,
        validity,
        last_check_at: None,
        last_refresh_at,
        refresh_failure_count: i64::from(last_failure_at.is_some()),
        last_failure_at,
        last_notified_failure_count: 0,
        reason_code: None,
    }
}

#[test]
fn next_renewal_counts_from_the_last_refresh_and_waits_after_a_failure() {
    const HOUR: i64 = 60 * 60 * 1000;
    const AGE: i64 = 4 * 24 * HOUR;
    let provider = Renewing::default();
    let renewable = profile_at(1_000, Some("passport"));
    let at = |health: Option<&CredentialProfileHealth>| {
        next_renewal_at(&provider, &renewable, health).unwrap()
    };

    // From the profile's last change until it is refreshed.
    assert_eq!(at(None), Some(1_000 + AGE));
    let refreshed = health_with(CredentialValidity::Valid, Some(50_000), None);
    assert_eq!(at(Some(&refreshed)), Some(50_000 + AGE));
    // A failed renewal pushes the next one an hour past the failure, but never
    // earlier than the age allows.
    let failed_late = health_with(
        CredentialValidity::NeedsRefresh,
        Some(50_000),
        Some(50_000 + AGE + 10),
    );
    assert_eq!(at(Some(&failed_late)), Some(50_000 + AGE + 10 + HOUR));
    let failed_early = health_with(CredentialValidity::NeedsRefresh, Some(50_000), Some(60_000));
    assert_eq!(at(Some(&failed_early)), Some(50_000 + AGE));

    // Never for an account that needs a new login, one the provider cannot
    // refresh, or a provider without a renewal age.
    let invalid = health_with(CredentialValidity::Invalid, Some(50_000), None);
    assert_eq!(at(Some(&invalid)), None);
    assert_eq!(
        next_renewal_at(&provider, &profile_at(1_000, None), None).unwrap(),
        None
    );
    assert_eq!(
        next_renewal_at(&super::super::CookieProvider, &renewable, None).unwrap(),
        None
    );
}

async fn last_used(repository: &CredentialProfileRepository, id: &str) -> Option<i64> {
    repository.get(id).await.unwrap().last_used_at
}

/// One account on a platform, selected as a fixed account, with the pool
/// behind the repository.
async fn fixed_account() -> (
    sqlx::SqlitePool,
    Arc<CredentialProfileRepository>,
    Arc<CredentialProviderRegistry>,
    CredentialProfile,
    ResolvedCredentialPolicy,
) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let repository = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
    let refresh = Arc::new(
        CredentialProviderRegistry::new()
            .with_admission(crate::credentials::test_support::unthrottled_admission()),
    );
    let owner = CredentialOwner::Platform {
        platform_id: "platform-huya".into(),
    };
    let account = repository
        .create(
            owner.id(),
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
    let policy = ResolvedCredentialPolicy::new(
        owner.id().into(),
        owner,
        CredentialSelection::Fixed {
            credential_id: account.id.clone(),
        },
    )
    .unwrap();
    (pool, repository, refresh, account, policy)
}

#[tokio::test]
async fn use_is_recorded_on_handover_coalesced_and_kept_across_revisions() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (pool, repository, refresh, account, policy) = fixed_account().await;
        let other = repository
            .create(
                "platform-huya",
                "B",
                true,
                &CredentialMaterial {
                    cookies: "account=B".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap();
        let service = CredentialExecutionService::new(repository.clone(), refresh.clone());
        let account = &account;
        let fresh = || CredentialExecutionService::new(repository.clone(), refresh.clone());
        let set_last_used = |at: i64| {
            let pool = pool.clone();
            let id = account.id.clone();
            async move {
                sqlx::query("UPDATE credential_profiles SET last_used_at = ? WHERE id = ?")
                    .bind(at)
                    .bind(id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };

        assert_eq!(last_used(&repository, &account.id).await, None);
        let before = crate::database::time::now_ms();
        assert_eq!(
            extract_cookies(&service, &policy).await.unwrap(),
            "account=A"
        );
        let first = last_used(&repository, &account.id).await.unwrap();
        assert!(first >= before);
        // Other accounts were never handed out.
        assert_eq!(last_used(&repository, &other.id).await, None);
        // A use is not an edit.
        let stored = repository.get(&account.id).await.unwrap();
        assert_eq!(
            (stored.revision, stored.version),
            (account.revision, account.version)
        );

        // This process writes each account at most once per window.
        set_last_used(1_000).await;
        extract_cookies(&service, &policy).await.unwrap();
        assert_eq!(last_used(&repository, &account.id).await, Some(1_000));

        // A recent stored use is kept even without the in-memory record...
        let recent = crate::database::time::now_ms() - 60_000;
        set_last_used(recent).await;
        extract_cookies(&fresh(), &policy).await.unwrap();
        assert_eq!(last_used(&repository, &account.id).await, Some(recent));
        // ...and an old one moves on.
        set_last_used(crate::database::time::now_ms() - 10 * 60_000).await;
        let before = crate::database::time::now_ms();
        extract_cookies(&fresh(), &policy).await.unwrap();
        let moved = last_used(&repository, &account.id).await.unwrap();
        assert!(moved >= before);

        // New material starts a new revision but keeps the last use.
        let replaced = repository
            .update(
                &account.id,
                account.version,
                None,
                None,
                Some(&CredentialMaterial {
                    cookies: "account=A2".into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                }),
                None,
            )
            .await
            .unwrap();
        assert!(replaced.revision > account.revision);
        assert_eq!(replaced.last_used_at, Some(moved));
    })
    .await
    .expect("last-use test timed out");
}

#[tokio::test]
async fn a_failed_use_record_does_not_fail_the_operation() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (pool, repository, refresh, account, policy) = fixed_account().await;
        let service = CredentialExecutionService::new(repository.clone(), refresh);
        sqlx::query("CREATE TRIGGER refuse_last_use BEFORE UPDATE OF last_used_at ON credential_profiles BEGIN SELECT RAISE(ABORT, 'last use refused'); END")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            repository
                .record_use(&account.id, crate::database::time::now_ms(), USE_RECORD_WINDOW)
                .await
                .is_err()
        );
        assert_eq!(extract_cookies(&service, &policy).await.unwrap(), "account=A");
        assert_eq!(last_used(&repository, &account.id).await, None);
    })
    .await
    .expect("failed last-use test timed out");
}
