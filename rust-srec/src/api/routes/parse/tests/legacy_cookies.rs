use super::*;

#[derive(Clone, Copy)]
enum RefreshResult {
    Noop,
    Success,
    Failure,
}

struct Provider {
    calls: std::sync::atomic::AtomicUsize,
    result: RefreshResult,
}

#[async_trait::async_trait]
impl crate::credentials::CredentialManager for Provider {
    fn platform_id(&self) -> &'static str {
        "bilibili"
    }
    async fn check_status(
        &self,
        cookies: &str,
    ) -> Result<crate::credentials::CredentialStatus, crate::credentials::CredentialError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(cookies, "parent=1");
        Ok(match self.result {
            RefreshResult::Noop => crate::credentials::CredentialStatus::Valid,
            _ => crate::credentials::CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            },
        })
    }
    async fn refresh(
        &self,
        state: &crate::credentials::RefreshState,
    ) -> Result<crate::credentials::RefreshedCredentials, crate::credentials::CredentialError> {
        assert_eq!(state.refresh_token.as_deref(), Some("parent-token"));
        if matches!(self.result, RefreshResult::Failure) {
            return Err(crate::credentials::CredentialError::InvalidRefreshToken);
        }
        Ok(crate::credentials::RefreshedCredentials {
            cookies: "fresh=1".into(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        })
    }
    async fn validate(&self, _: &str) -> Result<bool, crate::credentials::CredentialError> {
        Ok(true)
    }
}

async fn state(pool: &sqlx::SqlitePool) -> ParseRouteState {
    let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let streamer_manager = Arc::new(StreamerManager::new(
        streamer_repo.clone(),
        ConfigEventBroadcaster::new(),
    ));
    streamer_manager.hydrate().await.unwrap();
    let credentials = Arc::new(CredentialRefreshService::new(Arc::new(
        SqlxCredentialStore::new(pool.clone(), pool.clone()),
    )));
    ParseRouteState {
        auth_enabled: false,
        playback: Arc::new(crate::services::playback_context::PlaybackContextService::default()),
        execution: test_execution(pool),
        admission: credentials.admission(),
        config_service: Arc::new(ConfigService::new(
            Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
            streamer_repo,
        )),
        credential_service: credentials,
        streamer_manager,
    }
}

#[tokio::test]
async fn registered_and_unregistered_parse_keep_legacy_cookie_rules_and_raw_overrides() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let values = [None, Some(serde_json::Value::Null), Some(serde_json::json!("")), Some(serde_json::json!(" \t ")), Some(serde_json::json!("selected=1"))];
        for registered in [false, true] {
            for value in &values {
                let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
                run_migrations(&pool).await.unwrap();
                let scalar = value.as_ref().and_then(serde_json::Value::as_str);
                sqlx::query("UPDATE platform_config SET cookies = ?, platform_specific_config = '{\"refresh_token\":\"platform-token\",\"access_token\":\"platform-access\",\"quality\":80}' WHERE id = 'platform-bilibili'")
                    .bind(if registered { Some("parent=1") } else { scalar }).execute(&pool).await.unwrap();
                if registered {
                    let mut row = StreamerDbModel::new("Parse", STREAMER_URL, "platform-bilibili");
                    row.streamer_specific_config = value.as_ref().map(|cookie| serde_json::json!({"cookies":cookie,"refresh_token":"streamer-token"}).to_string());
                    SqlxStreamerRepository::new(pool.clone(), pool.clone()).create_streamer(&row).await.unwrap();
                }
                let state = state(&pool).await;
                let result = resolve_extractor_config_for_url(&state, STREAMER_URL, None, OperationDeadline::default()).await;
                let expected = if registered { scalar.or(Some("parent=1")) } else { scalar.filter(|s| !s.trim().is_empty()) };
                assert_eq!(result.cookies.as_deref(), expected, "registered={registered} / {value:?}");
                for raw in ["", " ", "explicit=1"] {
                    let result = resolve_extractor_config_for_url(&state, STREAMER_URL, Some(raw.into()), OperationDeadline::default()).await;
                    assert_eq!(result.cookies.as_deref(), Some(raw));
                    let extras = result.platform_extras.unwrap_or(serde_json::Value::Null).to_string();
                    assert!(!extras.contains("platform-token"));
                    assert!(!extras.contains("platform-access"));
                    assert!(!extras.contains("streamer-token"));
                }
                pool.close().await;
            }
        }
    }).await.expect("parse credential matrix must finish without provider network");
}

#[tokio::test]
async fn explicit_raw_cookie_parse_never_borrows_stored_soop_login() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        sqlx::query("UPDATE platform_config SET cookies = 'stored=1', platform_specific_config = '{\"username\":\"stored-user\",\"password\":\"stored-password\",\"stream_password\":\"room\"}' WHERE id = 'platform-soop'").execute(&pool).await.unwrap();
        let state = state(&pool).await;
        for cookies in ["", "raw=1"] {
            let resolved = resolve_extractor_config_for_url(&state, "https://play.sooplive.co.kr/test", Some(cookies.into()), OperationDeadline::default()).await;
            assert_eq!(resolved.cookies.as_deref(), Some(cookies));
            let extras = resolved.platform_extras.unwrap();
            assert!(extras.get("username").is_none());
            assert!(extras.get("password").is_none());
            assert_eq!(extras["stream_password"], "room");
        }
        pool.close().await;
    }).await.expect("raw SOOP parse must finish without login");
}

#[tokio::test]
async fn registered_parse_refresh_replaces_blank_only_on_success_and_raw_input_skips_provider() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for result in [RefreshResult::Noop, RefreshResult::Success, RefreshResult::Failure] {
            let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
            run_migrations(&pool).await.unwrap();
            sqlx::query("UPDATE platform_config SET cookies = 'parent=1', platform_specific_config = '{\"refresh_token\":\"parent-token\"}' WHERE id = 'platform-bilibili'").execute(&pool).await.unwrap();
            let mut row = StreamerDbModel::new("Blank override", STREAMER_URL, "platform-bilibili");
            row.streamer_specific_config = Some(serde_json::json!({"cookies":" "}).to_string());
            SqlxStreamerRepository::new(pool.clone(), pool.clone()).create_streamer(&row).await.unwrap();
            let mut state = state(&pool).await;
            let provider = Arc::new(Provider { calls: std::sync::atomic::AtomicUsize::new(0), result });
            let mut credentials = CredentialRefreshService::new(Arc::new(SqlxCredentialStore::new(pool.clone(), pool.clone())));
            credentials.register_manager(provider.clone());
            state.admission = credentials.admission();
            state.credential_service = Arc::new(credentials);
            for raw in ["", "explicit=1"] {
                let resolved = resolve_extractor_config_for_url(&state, STREAMER_URL, Some(raw.into()), OperationDeadline::default()).await;
                assert_eq!(resolved.cookies.as_deref(), Some(raw));
            }
            assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
            let resolved = resolve_extractor_config_for_url(&state, STREAMER_URL, None, OperationDeadline::default()).await;
            assert_eq!(resolved.cookies.as_deref(), Some(if matches!(result, RefreshResult::Success) { "fresh=1" } else { " " }));
            assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            pool.close().await;
        }
    }).await.expect("legacy parse refresh fixture must finish without network");
}
