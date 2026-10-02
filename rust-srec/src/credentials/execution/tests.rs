use super::*;
use crate::credentials::CredentialOwner;
use crate::database::repositories::SqlxCredentialStore;
use crate::monitor::{RateLimiterConfig, RateLimiterManager};

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
    let admission = Arc::new(super::super::PlatformAdmission::new(
        RateLimiterManager::with_config(RateLimiterConfig {
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
    let service = CredentialExecutionService::new(repository.clone(), refresh);
    let owner = CredentialOwner::Platform {
        platform_id: "platform-huya".to_string(),
    };
    let mut profiles = Vec::new();
    for label in ["A", "B", "C"] {
        profiles.push(
            repository
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
async fn independent_rotation_and_bound_checks_do_not_share_cursor_slots() {
    let (repository, service, policy, profiles) = fixture().await;
    let first = selected(&service, &policy, None).await;
    assert_eq!(first.value, "account=A");
    repository
        .accessible(&policy.owner, &policy.platform_id)
        .await
        .unwrap();
    repository.health(&profiles[0].id).await.unwrap();
    assert_eq!(
        selected(&service, &policy, Some(&first.snapshot.binding))
            .await
            .value,
        "account=A"
    );
    assert_eq!(selected(&service, &policy, None).await.value, "account=B");
    assert_eq!(selected(&service, &policy, None).await.value, "account=C");
    assert_eq!(selected(&service, &policy, None).await.value, "account=A");
}

#[tokio::test]
async fn typed_authentication_fails_over_but_fixed_never_changes_accounts() {
    let (repository, service, mut policy, profiles) = fixture().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let result = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            |snapshot| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if snapshot.material.cookies == "account=A" {
                        Err(Error::Extractor(ExtractorError::Authentication {
                            code: "expired".to_string(),
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
    assert_eq!(result.value, "account=B");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(
        repository
            .health(&profiles[0].id)
            .await
            .unwrap()
            .unwrap()
            .validity,
        "invalid"
    );
    policy = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    let error = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::default(),
            |_| async {
                Ok(Extracted {
                    preserve_health: false,
                    value: (),
                    session_cookies: None,
                })
            },
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::CredentialUnavailable(_)));
}

#[tokio::test]
async fn unrecoverable_profile_authentication_emits_one_identified_invalid_notification() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (_repository, service, policy, profiles) = fixture().await;
        let notifications = Arc::new(crate::notification::NotificationService::new());
        let mut received = notifications.subscribe();
        service
            .refresh
            .set_notification_service(notifications.clone());
        service
            .execute(
                &policy,
                None,
                true,
                OperationDeadline::default(),
                |snapshot| async move {
                    if snapshot.material.cookies == "account=A" {
                        Err(Error::Extractor(ExtractorError::Authentication {
                            code: "expired".into(),
                        }))
                    } else {
                        Ok(Extracted {
                            preserve_health: false,
                            value: (),
                            session_cookies: None,
                        })
                    }
                },
            )
            .await
            .unwrap();
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
                assert_eq!(scope.record_id(), policy.platform_id);
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
                    OperationDeadline::default(),
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
        .execute(&policy, None, true, OperationDeadline::default(), |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async {
                Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::RateLimited {
                    scope: ThrottleScope::Unknown,
                    code: None,
                    retry_after: Some(Duration::from_millis(1)),
                }))
            }
        })
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
async fn disabled_profiles_are_skipped_even_when_failover_is_off() {
    let (repository, service, policy, profiles) = fixture().await;
    repository
        .update(&profiles[0].id, 1, None, Some(false), None)
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
        .execute(&policy, None, true, OperationDeadline::default(), |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async {
                Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::Authentication {
                    code: "expired".into(),
                }))
            }
        })
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
                |_| async {
                    Err::<Extracted<()>, _>(Error::Other("unexpected alternate account".into()))
                }
            )
            .await,
        Err(Error::CredentialUnavailable(_))
    ));
}

#[tokio::test]
async fn cancelled_half_open_probe_releases_ownership_without_poisoning_health() {
    let (repository, service, policy, profiles) = fixture().await;
    repository
        .publish_health(
            &profiles[0],
            "valid",
            Some(crate::database::time::now_ms() - 1),
            Some("account_throttled"),
            false,
        )
        .await
        .unwrap();
    let policy = ResolvedCredentialPolicy::new(
        policy.platform_id,
        policy.owner,
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    let result = service
        .execute(
            &policy,
            None,
            true,
            OperationDeadline::new(Duration::from_millis(20)),
            |_| async { std::future::pending::<Result<Extracted<()>>>().await },
        )
        .await;
    assert!(matches!(result, Err(Error::Monitor(_))));
    assert!(service.probes.lock().is_empty());
    assert_eq!(selected(&service, &policy, None).await.value, "account=A");
    assert_eq!(
        repository
            .health(&profiles[0].id)
            .await
            .unwrap()
            .unwrap()
            .cooldown_until,
        None
    );
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
        .publish_health(&profiles[1], "invalid", None, Some("login_required"), false)
        .await
        .unwrap();
    let result = service
        .execute(
            &policy,
            Some(&bound),
            true,
            OperationDeadline::default(),
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
        .update(&profiles[0].id, 1, None, Some(false), None)
        .await
        .unwrap();
    assert!(
        service
            .execute(
                &fixed,
                None,
                false,
                OperationDeadline::default(),
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
        "profiles_disabled"
    );
    assert_eq!(
        service.unavailable_status(&fixed).unwrap().reason,
        "profiles_disabled"
    );
    repository
        .update(&profiles[0].id, 2, None, Some(true), None)
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
        "unknown",
        "unsupported validation must not invent a validated conclusion"
    );
}

struct RecordingRefresh {
    inputs: Arc<Mutex<Vec<RefreshState>>>,
}

#[async_trait::async_trait]
impl super::super::CredentialManager for RecordingRefresh {
    fn platform_id(&self) -> &'static str {
        "huya"
    }
    async fn check_status(
        &self,
        _cookies: &str,
    ) -> std::result::Result<CredentialStatus, CredentialError> {
        Ok(CredentialStatus::Valid)
    }
    async fn refresh(
        &self,
        state: &RefreshState,
    ) -> std::result::Result<RefreshedCredentials, CredentialError> {
        self.inputs.lock().push(state.clone());
        Ok(RefreshedCredentials {
            cookies: "account=repaired".into(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        })
    }
    async fn validate(&self, _cookies: &str) -> std::result::Result<bool, CredentialError> {
        Ok(true)
    }
}

#[tokio::test]
async fn repair_uses_bound_access_token_and_consumes_another_attempt() {
    let (repository, mut service, policy, profiles) = fixture().await;
    let inputs = Arc::new(Mutex::new(Vec::new()));
    Arc::get_mut(&mut service.refresh)
        .unwrap()
        .register_manager(Arc::new(RecordingRefresh {
            inputs: inputs.clone(),
        }));
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
    assert_eq!(
        inputs.lock()[0].extra.as_ref().unwrap()["access_token"],
        "access-a"
    );
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
impl super::super::CredentialManager for GatedRefresh {
    fn platform_id(&self) -> &'static str {
        "huya"
    }
    async fn check_status(
        &self,
        _cookies: &str,
    ) -> std::result::Result<CredentialStatus, CredentialError> {
        Ok(CredentialStatus::NeedsRefresh {
            refresh_deadline: None,
        })
    }
    async fn refresh(
        &self,
        _state: &RefreshState,
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
    async fn validate(&self, _cookies: &str) -> std::result::Result<bool, CredentialError> {
        Ok(true)
    }
}

#[tokio::test]
async fn profile_refresh_allows_label_edits_but_rejects_replaced_account_material() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for replacement in [false, true] {
            let (repository, mut service, _policy, profiles) = fixture().await;
            let manager = Arc::new(GatedRefresh { started: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0) });
            Arc::get_mut(&mut service.refresh).unwrap().register_manager(manager.clone());
            let operation = service.refresh_profile(&profiles[0].id, OperationDeadline::default()); tokio::pin!(operation);
            tokio::select! { _ = &mut operation => panic!("provider must wait"), _ = manager.started.notified() => {} }
            let replacement_material = CredentialMaterial { cookies: "account=user-replaced".into(), refresh_token: None, access_token: None, reauth_config: None };
            repository.update(&profiles[0].id, 1, Some("Renamed"), None, replacement.then_some(&replacement_material)).await.unwrap();
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
        for cooldown in [false, true] {
            let (repository, service, policy, profiles) = fixture().await;
            service
                .refresh
                .admission()
                .defer(&policy.platform_id, Duration::from_millis(500));
            let used = Arc::new(Mutex::new(Vec::new()));
            let captured = used.clone();
            let operation = service.execute(
                &policy,
                None,
                true,
                OperationDeadline::default(),
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
                    if cooldown { "unknown" } else { "invalid" },
                    cooldown.then(|| crate::database::time::now_ms() + 60_000),
                    Some(if cooldown {
                        "account_throttled"
                    } else {
                        "login_required"
                    }),
                    false,
                )
                .await
                .unwrap();
            let result = operation.await.unwrap();
            assert_eq!(result.value, "account=B");
            assert_eq!(*used.lock(), vec!["account=B"]);
        }
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
impl super::super::CredentialManager for UnchangedRefresh {
    fn platform_id(&self) -> &'static str {
        "huya"
    }
    async fn check_status(
        &self,
        _cookies: &str,
    ) -> std::result::Result<CredentialStatus, CredentialError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(CredentialStatus::Valid)
    }
    async fn refresh(
        &self,
        state: &RefreshState,
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
    async fn validate(&self, _cookies: &str) -> std::result::Result<bool, CredentialError> {
        Ok(true)
    }
}

#[tokio::test]
async fn unchanged_refresh_keeps_the_revision_bound_recordings_rely_on() {
    let (repository, mut service, _policy, profiles) = fixture().await;
    Arc::get_mut(&mut service.refresh)
        .unwrap()
        .register_manager(Arc::new(UnchangedRefresh {
            calls: Default::default(),
        }));
    let profile = &profiles[0];
    repository
        .publish_health(
            profile,
            "needs_refresh",
            None,
            Some("authentication_failed"),
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
    assert_eq!(health.validity, "valid");
    assert!(health.last_refresh_at.is_some());
    assert_eq!(health.reason_code, None);
}

#[tokio::test]
async fn disabled_profiles_reject_manual_actions_before_any_provider_call() {
    let (repository, mut service, _policy, profiles) = fixture().await;
    let manager = Arc::new(UnchangedRefresh {
        calls: Default::default(),
    });
    Arc::get_mut(&mut service.refresh)
        .unwrap()
        .register_manager(manager.clone());
    repository
        .update(&profiles[0].id, 1, None, Some(false), None)
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
async fn an_offline_answer_to_the_half_open_probe_ends_the_cooldown() {
    let (repository, service, policy, profiles) = fixture().await;
    let policy = ResolvedCredentialPolicy::new(
        policy.platform_id.clone(),
        policy.owner.clone(),
        CredentialSelection::Fixed {
            credential_id: profiles[0].id.clone(),
        },
    )
    .unwrap();
    repository
        .publish_health(
            &profiles[0],
            "valid",
            Some(crate::database::time::now_ms() - 1),
            Some("account_throttled"),
            false,
        )
        .await
        .unwrap();
    service
        .execute(
            &policy,
            None,
            false,
            OperationDeadline::new(Duration::from_secs(5)),
            |_| async {
                Ok(Extracted {
                    preserve_health: true,
                    value: (),
                    session_cookies: None,
                })
            },
        )
        .await
        .unwrap();
    let health = repository.health(&profiles[0].id).await.unwrap().unwrap();
    assert_eq!(health.cooldown_until, None);
    assert_eq!(health.throttle_count, 0);
    assert_eq!(health.validity, "valid");
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
            |_| async {
                Err::<Extracted<()>, _>(Error::Extractor(ExtractorError::RateLimited {
                    scope: ThrottleScope::Platform,
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
            .refresh
            .admission()
            .admit(
                &anonymous.platform_id,
                OperationDeadline::new(Duration::from_millis(200))
            )
            .await
            .is_err(),
        "managed and legacy callers share the platform backoff"
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
            |_| async { Err::<Extracted<()>, _>(Error::Other("no candidate may be tried".into())) },
        )
    };
    repository
        .publish_health(&profiles[0], "invalid", None, Some("login_required"), false)
        .await
        .unwrap();
    repository
        .update(&profiles[1].id, 1, None, Some(false), None)
        .await
        .unwrap();
    let until = crate::database::time::now_ms() + 60_000;
    repository
        .publish_health(
            &profiles[2],
            "valid",
            Some(until),
            Some("account_throttled"),
            false,
        )
        .await
        .unwrap();
    assert!(attempt().await.is_err());
    assert_eq!(reason(&service).as_deref(), Some("cooling_down"));
    assert_eq!(
        service.unavailable_status(&policy).unwrap().retry_at,
        Some(until)
    );

    repository
        .publish_health(&profiles[2], "invalid", None, Some("login_required"), false)
        .await
        .unwrap();
    assert!(attempt().await.is_err());
    assert_eq!(reason(&service).as_deref(), Some("login_required"));

    for profile in [&profiles[0], &profiles[2]] {
        let current = repository.get(&profile.id).await.unwrap();
        repository
            .update(&profile.id, current.version, None, Some(false), None)
            .await
            .unwrap();
    }
    assert!(attempt().await.is_err());
    assert_eq!(reason(&service).as_deref(), Some("profiles_disabled"));
}

/// The plan's acceptance storyline at the execution boundary: A/B/C rotate
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
            expired_b,
        )
        .await
        .unwrap();
    assert_eq!(recovered.value, "account=C");
    assert_eq!(identity(&recovered.snapshot), profiles[2].id);
    assert_eq!(recovered.snapshot.binding.epoch, pinned.epoch);

    // A targeted refresh of A changes only A.
    let inputs = Arc::new(Mutex::new(Vec::new()));
    Arc::get_mut(&mut service.refresh)
        .unwrap()
        .register_manager(Arc::new(RecordingRefresh {
            inputs: inputs.clone(),
        }));
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
