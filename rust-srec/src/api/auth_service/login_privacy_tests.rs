use super::*;
use crate::api::error::ApiError;
use crate::database::models::UserDbModel;
use crate::database::repositories::{
    SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
};

async fn service() -> (sqlx::SqlitePool, AuthService) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let users = Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone()));
    let hash = tokio::task::spawn_blocking(|| AuthService::hash_password("correct-password"))
        .await
        .unwrap()
        .unwrap();
    let actual = PasswordHash::new(&hash).unwrap();
    let dummy = PasswordHash::new(LOGIN_DUMMY_PASSWORD_HASH).unwrap();
    assert_eq!(dummy.algorithm, actual.algorithm);
    assert_eq!(dummy.version, actual.version);
    assert_eq!(dummy.params.to_string(), actual.params.to_string());
    assert_eq!(
        dummy.hash.unwrap().as_bytes().len(),
        actual.hash.unwrap().as_bytes().len()
    );
    for (name, active) in [("active", true), ("disabled", false)] {
        let mut user = UserDbModel::new(name, &hash, vec!["user".to_string()]);
        user.is_active = active;
        users.create(&user).await.unwrap();
    }
    let service = AuthService::new(
        users,
        Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone())),
        Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool.clone())),
        Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test",
            "test",
            Some(900),
        )),
        AuthConfig::default(),
    )
    .with_login_rate_limiter(LoginRateLimiter::new(
        10,
        100,
        std::time::Duration::from_secs(60),
        100,
    ));
    (pool, service)
}

#[tokio::test]
async fn invalid_logins_share_password_work_and_public_error_for_missing_active_and_disabled_users()
{
    let (pool, service) = service().await;
    let mut expected = None;
    for name in ["missing", "active", "disabled"] {
        let before = service.password_verify_calls.load(Ordering::SeqCst);
        let error = service
            .authenticate(name, "incorrect-password", None, None)
            .await
            .unwrap_err();
        assert!(matches!(error, AuthError::InvalidCredentials));
        assert_eq!(
            service.password_verify_calls.load(Ordering::SeqCst),
            before + 1
        );
        let error = ApiError::from(error);
        let public = (error.status, error.code, error.message);
        assert_eq!(public.0, axum::http::StatusCode::UNAUTHORIZED);
        if let Some(expected) = &expected {
            assert_eq!(&public, expected);
        } else {
            expected = Some(public);
        }
    }
    let error = service
        .authenticate("disabled", "correct-password", None, None)
        .await
        .unwrap_err();
    assert!(matches!(error, AuthError::AccountDisabled));
    assert_eq!(
        ApiError::from(error).status,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_eq!(service.password_verify_calls.load(Ordering::SeqCst), 4);
    assert!(
        service
            .authenticate("active", "correct-password", None, None)
            .await
            .is_ok()
    );
    pool.close().await;
}

#[tokio::test]
async fn missing_user_obeys_password_worker_admission_and_throttling_skips_work() {
    let (pool, mut service) = service().await;
    service.password_work_permits = Arc::new(Semaphore::new(1));
    service.login_rate_limiter =
        LoginRateLimiter::new(1, 100, std::time::Duration::from_secs(60), 100);
    let permit = service.password_work_permits.acquire().await.unwrap();
    let mut attempt = Box::pin(service.authenticate("missing", "incorrect", None, None));
    assert!(futures::poll!(attempt.as_mut()).is_pending());
    assert_eq!(service.password_verify_calls.load(Ordering::SeqCst), 0);
    drop(permit);
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), attempt)
            .await
            .unwrap(),
        Err(AuthError::InvalidCredentials)
    ));
    assert_eq!(service.password_verify_calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        service
            .authenticate("missing", "incorrect", None, None)
            .await,
        Err(AuthError::TooManyAttempts { .. })
    ));
    assert_eq!(service.password_verify_calls.load(Ordering::SeqCst), 1);
    pool.close().await;
}

#[tokio::test]
async fn long_device_descriptions_are_bounded_without_rejecting_login() {
    let (pool, service) = service().await;
    for device in [
        None,
        Some(String::new()),
        Some("😀é".repeat(MAX_DEVICE_INFO_LENGTH)),
    ] {
        let expected = device.as_ref().map(|value| {
            value
                .chars()
                .take(MAX_DEVICE_INFO_LENGTH)
                .collect::<String>()
        });
        let response = service
            .authenticate("active", "correct-password", device, None)
            .await
            .unwrap();
        let stored = service
            .token_repo
            .find_by_token_hash(&AuthService::hash_token(&response.refresh_token))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.device_info, expected);
        assert!(
            stored
                .device_info
                .as_ref()
                .is_none_or(|value| value.chars().count() <= MAX_DEVICE_INFO_LENGTH)
        );
    }
    pool.close().await;
}

#[derive(Clone)]
struct CapturedLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn legacy_device_metadata_is_bounded_for_refresh_session_listing_and_logout_logs() {
    use tracing::instrument::WithSubscriber;
    let (pool, service) = service().await;
    let login = service
        .authenticate("active", "correct-password", None, None)
        .await
        .unwrap();
    let original = service
        .token_repo
        .find_by_token_hash(&AuthService::hash_token(&login.refresh_token))
        .await
        .unwrap()
        .unwrap();
    let legacy = format!(
        "{}LEGACY_SUFFIX_MUST_NOT_BE_LOGGED",
        "界😀".repeat(MAX_DEVICE_INFO_LENGTH)
    );
    let expected = bounded_device_info(Some(legacy.clone()));
    sqlx::query("UPDATE refresh_tokens SET device_info = ? WHERE id = ?")
        .bind(&legacy)
        .bind(&original.id)
        .execute(&pool)
        .await
        .unwrap();
    let captured = CapturedLog(Arc::new(std::sync::Mutex::new(Vec::new())));
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    async {
        let sessions = service
            .list_active_sessions(&original.user_id)
            .await
            .unwrap();
        assert_eq!(sessions[0].device_info, expected);
        let refreshed = service.refresh_tokens(&login.refresh_token).await.unwrap();
        let successor = service
            .token_repo
            .find_by_token_hash(&AuthService::hash_token(&refreshed.refresh_token))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(successor.device_info, expected);
        sqlx::query("UPDATE refresh_tokens SET device_info = ?, expires_at = 0 WHERE id = ?")
            .bind(&legacy)
            .bind(&successor.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            service.refresh_tokens(&refreshed.refresh_token).await,
            Err(AuthError::TokenExpired)
        ));
        service.logout(&refreshed.refresh_token).await.unwrap();
        assert!(
            service
                .token_repo
                .find_by_token_hash(&AuthService::hash_token(&refreshed.refresh_token))
                .await
                .unwrap()
                .unwrap()
                .is_revoked()
        );
    }
    .with_subscriber(subscriber)
    .await;
    let output = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    for event in [
        "Token refresh succeeded",
        "Expired refresh token presented",
        "Logout successful",
    ] {
        assert!(output.contains(event), "missing captured event {event}");
    }
    assert!(!output.contains("LEGACY_SUFFIX_MUST_NOT_BE_LOGGED"));
    pool.close().await;
}
