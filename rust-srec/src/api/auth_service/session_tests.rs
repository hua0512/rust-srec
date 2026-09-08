use super::*;
use crate::database::models::UserDbModel;
use crate::database::repositories::{
    SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
};
use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const SESSION_MIGRATION: i64 = 20260908010000;
const PASSWORD: &str = "OriginalPass123";

struct Fixture {
    pool: SqlitePool,
    service: AuthService,
    tokens: Arc<SqlxRefreshTokenRepository>,
    user: UserDbModel,
}

impl Fixture {
    async fn new(config: AuthConfig) -> Self {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        let users = Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone()));
        let mut user = UserDbModel::new(
            "session-user",
            AuthService::hash_password(PASSWORD).unwrap(),
            vec!["user".to_owned()],
        );
        user.must_change_password = false;
        users.create(&user).await.unwrap();
        let tokens = Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone()));
        let service = AuthService::new(
            users,
            tokens.clone(),
            Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool.clone())),
            Arc::new(JwtService::new(
                "session-test-secret-key-32-chars!!",
                "test",
                "test",
                Some(config.access_token_expiration_secs),
            )),
            config,
        );
        Self {
            pool,
            service,
            tokens,
            user,
        }
    }

    async fn login(&self) -> AuthResponse {
        self.service
            .authenticate(&self.user.username, PASSWORD, None, None)
            .await
            .unwrap()
    }

    fn session_id(&self, response: &AuthResponse) -> String {
        self.service
            .jwt_service
            .validate_token(&response.access_token)
            .unwrap()
            .sid
            .unwrap()
    }

    async fn assert_authorized(&self, response: &AuthResponse) {
        self.service
            .authorize_access_token(&response.access_token, false)
            .await
            .unwrap();
    }

    async fn assert_revoked(&self, response: &AuthResponse) {
        assert!(matches!(
            self.service
                .authorize_access_token(&response.access_token, false)
                .await,
            Err(AuthError::TokenRevoked)
        ));
    }
}

#[tokio::test]
async fn bound_sessions_reject_legacy_access_and_fail_closed_on_lookup_failure() {
    let fixture = Fixture::new(AuthConfig::default()).await;
    let login = fixture.login().await;
    fixture.assert_authorized(&login).await;
    let legacy = fixture
        .service
        .jwt_service
        .generate_token(&fixture.user.id, vec!["user".to_owned()])
        .unwrap();
    assert!(
        fixture
            .service
            .jwt_service
            .validate_token(&legacy)
            .unwrap()
            .sid
            .is_none()
    );
    assert!(matches!(
        fixture.service.authorize_access_token(&legacy, false).await,
        Err(AuthError::InvalidToken)
    ));
    fixture.pool.close().await;
    assert!(matches!(
        fixture
            .service
            .authorize_access_token(&login.access_token, false)
            .await,
        Err(AuthError::Database(_))
    ));
}

#[tokio::test]
async fn single_logout_closes_rotated_lineage_without_revoking_other_devices() {
    let fixture = Fixture::new(AuthConfig::default()).await;
    let first = fixture.login().await;
    let second = fixture.login().await;
    let rotated = fixture
        .service
        .refresh_tokens(&first.refresh_token)
        .await
        .unwrap();
    assert_eq!(fixture.session_id(&first), fixture.session_id(&rotated));
    fixture.assert_authorized(&first).await;
    fixture.service.logout(&first.refresh_token).await.unwrap();
    fixture.assert_revoked(&first).await;
    fixture.assert_revoked(&rotated).await;
    for token in [&first.refresh_token, &rotated.refresh_token] {
        assert!(matches!(
            fixture.service.refresh_tokens(token).await,
            Err(AuthError::TokenRevoked)
        ));
        fixture.assert_authorized(&second).await;
    }
    // Covers logout winning after a reuse detector's read but before its write lock.
    fixture
        .tokens
        .revoke_all_for_active_session(&fixture.user.id, &fixture.session_id(&first))
        .await
        .unwrap();
    fixture.assert_authorized(&second).await;
    fixture.service.logout_all(&fixture.user.id).await.unwrap();
    fixture.assert_revoked(&second).await;
}

#[tokio::test]
async fn active_rotation_replay_revokes_all_sessions_by_default() {
    let fixture = Fixture::new(AuthConfig::default()).await;
    let first = fixture.login().await;
    let other = fixture.login().await;
    let refreshed = fixture
        .service
        .refresh_tokens(&first.refresh_token)
        .await
        .unwrap();
    assert!(matches!(
        fixture.service.refresh_tokens(&first.refresh_token).await,
        Err(AuthError::TokenRevoked)
    ));
    fixture.assert_revoked(&first).await;
    fixture.assert_revoked(&other).await;
    fixture.assert_revoked(&refreshed).await;
    assert_eq!(
        fixture
            .tokens
            .count_active_by_user(&fixture.user.id)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn concurrent_refresh_has_one_successor_and_honors_reuse_policy() {
    for (revoke, grace, expected_revocation) in
        [(true, 0, true), (false, 0, false), (true, 60, false)]
    {
        let fixture = Fixture::new(AuthConfig {
            revoke_all_on_refresh_token_reuse: revoke,
            refresh_token_reuse_grace_secs: grace,
            ..AuthConfig::default()
        })
        .await;
        let first = fixture.login().await;
        let other = fixture.login().await;
        let (left, right) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(
                fixture.service.refresh_tokens(&first.refresh_token),
                fixture.service.refresh_tokens(&first.refresh_token)
            )
        })
        .await
        .unwrap();
        let winner = match (left, right) {
            (Ok(winner), Err(AuthError::TokenRevoked))
            | (Err(AuthError::TokenRevoked), Ok(winner)) => winner,
            results => panic!("expected one rotation winner: {results:?}"),
        };
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM refresh_tokens WHERE session_id = ?")
                .bind(fixture.session_id(&first))
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(count, 2, "one predecessor and exactly one successor");
        if expected_revocation {
            fixture.assert_revoked(&winner).await;
            fixture.assert_revoked(&other).await;
        } else {
            fixture.assert_authorized(&winner).await;
            fixture.assert_authorized(&other).await;
        }
    }
}

#[tokio::test]
async fn password_change_trigger_is_atomic_and_stale_login_cannot_create_session() {
    let fixture = Fixture::new(AuthConfig::default()).await;
    let login = fixture.login().await;
    let session_id = fixture.session_id(&login);
    let mut tx = fixture.pool.begin().await.unwrap();
    sqlx::query("UPDATE users SET password_hash = 'replacement' WHERE id = ?")
        .bind(&fixture.user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM auth_sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(revoked.is_some());
    tx.rollback().await.unwrap();
    fixture.assert_authorized(&login).await;
    fixture
        .service
        .change_password(&fixture.user.id, PASSWORD, "ChangedPass456")
        .await
        .unwrap();
    fixture.assert_revoked(&login).await;
    assert!(matches!(
        fixture.service.refresh_tokens(&login.refresh_token).await,
        Err(AuthError::TokenRevoked)
    ));
    // The login verified this old hash before the password write acquired its lock.
    let mut token = RefreshTokenDbModel::new(
        &fixture.user.id,
        "stale-login-token",
        Utc::now() + Duration::days(1),
        None,
    );
    token.session_id = Some("stale-login-session".to_owned());
    let session = AuthSessionDbModel {
        id: "stale-login-session".to_owned(),
        user_id: fixture.user.id.clone(),
        created_at: token.created_at,
        expires_at: token.expires_at,
        revoked_at: None,
    };
    assert!(
        !fixture
            .tokens
            .create_session(&session, &token, &fixture.user.password_hash)
            .await
            .unwrap()
    );
    assert!(
        fixture
            .tokens
            .find_session(&fixture.user.id, &session.id)
            .await
            .unwrap()
            .is_none()
    );
    let fresh = fixture
        .service
        .authenticate(&fixture.user.username, "ChangedPass456", None, None)
        .await
        .unwrap();
    fixture.assert_authorized(&fresh).await;
}

#[tokio::test]
async fn disabled_account_cannot_restore_old_session_or_finish_stale_login() {
    let fixture = Fixture::new(AuthConfig::default()).await;
    let login = fixture.login().await;
    fixture.assert_authorized(&login).await;
    sqlx::query("UPDATE users SET is_active = 0 WHERE id = ?")
        .bind(&fixture.user.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    fixture.assert_revoked(&login).await;
    let mut token = RefreshTokenDbModel::new(
        &fixture.user.id,
        "disabled-login",
        Utc::now() + Duration::days(1),
        None,
    );
    token.session_id = Some("disabled-session".to_owned());
    let session = AuthSessionDbModel {
        id: "disabled-session".to_owned(),
        user_id: fixture.user.id.clone(),
        created_at: token.created_at,
        expires_at: token.expires_at,
        revoked_at: None,
    };
    assert!(
        !fixture
            .tokens
            .create_session(&session, &token, &fixture.user.password_hash)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE users SET is_active = 1 WHERE id = ?")
        .bind(&fixture.user.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    fixture.assert_revoked(&login).await;
}

#[tokio::test]
async fn rotation_failure_rolls_back_predecessor_and_session_expiry_covers_access() {
    let fixture = Fixture::new(AuthConfig {
        access_token_expiration_secs: 86400,
        refresh_token_expiration_secs: 60,
        ..AuthConfig::default()
    })
    .await;
    let login = fixture.login().await;
    let claims = fixture
        .service
        .jwt_service
        .validate_token(&login.access_token)
        .unwrap();
    let session_id = fixture.session_id(&login);
    let original = fixture
        .tokens
        .find_by_token_hash(&AuthService::hash_token(&login.refresh_token))
        .await
        .unwrap()
        .unwrap();
    let session = fixture
        .tokens
        .find_session(&fixture.user.id, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(session.expires_at >= i64::try_from(claims.exp * 1000).unwrap());
    let mut duplicate = original.clone();
    duplicate.id = uuid::Uuid::new_v4().to_string();
    assert!(
        fixture
            .tokens
            .rotate_session(&original.id, &duplicate, session.expires_at + 1000)
            .await
            .is_err()
    );
    let unchanged = fixture
        .tokens
        .find_by_token_hash(&original.token_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(unchanged.revoked_at.is_none());
    assert_eq!(
        fixture
            .tokens
            .find_session(&fixture.user.id, &session_id)
            .await
            .unwrap()
            .unwrap()
            .expires_at,
        session.expires_at
    );
    let refreshed = fixture
        .service
        .refresh_tokens(&login.refresh_token)
        .await
        .unwrap();
    let refreshed_claims = fixture
        .service
        .jwt_service
        .validate_token(&refreshed.access_token)
        .unwrap();
    assert!(
        fixture
            .tokens
            .find_session(&fixture.user.id, &session_id)
            .await
            .unwrap()
            .unwrap()
            .expires_at
            >= i64::try_from(refreshed_claims.exp * 1000).unwrap()
    );
}

#[tokio::test]
async fn auth_session_migration_preserves_legacy_refresh_and_integrity() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < SESSION_MIGRATION)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .run(&pool)
    .await
    .unwrap();
    let users = Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone()));
    let mut user = UserDbModel::new("legacy-session-user", "hash", vec!["user".to_owned()]);
    user.must_change_password = false;
    users.create(&user).await.unwrap();
    let now = crate::database::time::now_ms();
    for (id, expiry, revoked) in [
        ("legacy-valid", now + 60000, None),
        ("legacy-revoked", now + 60000, Some(now - 1000)),
        ("legacy-expired", now - 1000, None),
    ] {
        sqlx::query("INSERT INTO refresh_tokens(id,user_id,token_hash,created_at,expires_at,revoked_at,device_info) VALUES(?,?,?,?,?,?,?)")
            .bind(id).bind(&user.id).bind(AuthService::hash_token(id)).bind(now - 2000).bind(expiry).bind(revoked).bind("legacy device").execute(&pool).await.unwrap();
    }
    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let mapped: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refresh_tokens r JOIN auth_sessions s ON s.id = r.session_id WHERE s.user_id = r.user_id AND s.expires_at = r.expires_at AND s.created_at = r.created_at AND s.revoked_at IS r.revoked_at").fetch_one(&pool).await.unwrap();
    assert_eq!(mapped, 3);
    let tokens = Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone()));
    let service = AuthService::new(
        users.clone(),
        tokens,
        Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool.clone())),
        Arc::new(JwtService::new(
            "legacy-migration-secret-32-chars!!",
            "test",
            "test",
            None,
        )),
        AuthConfig::default(),
    );
    let upgraded = service.refresh_tokens("legacy-valid").await.unwrap();
    assert_eq!(
        service
            .jwt_service
            .validate_token(&upgraded.access_token)
            .unwrap()
            .sid
            .as_deref(),
        Some("legacy-valid")
    );
    service
        .authorize_access_token(&upgraded.access_token, false)
        .await
        .unwrap();
    assert!(matches!(
        service.refresh_tokens("legacy-revoked").await,
        Err(AuthError::TokenRevoked)
    ));
    service
        .authorize_access_token(&upgraded.access_token, false)
        .await
        .unwrap();
    assert!(matches!(
        service.refresh_tokens("legacy-expired").await,
        Err(AuthError::TokenExpired)
    ));
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    users.delete(&user.id).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM refresh_tokens")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn logout_can_close_access_session_after_refresh_cleanup() {
    let fixture = Fixture::new(AuthConfig {
        access_token_expiration_secs: 86400,
        refresh_token_expiration_secs: 60,
        ..AuthConfig::default()
    })
    .await;
    let login = fixture.login().await;
    let other = fixture.login().await;
    let principal = fixture
        .service
        .authorize_credential(&login.access_token, false)
        .await
        .unwrap();
    sqlx::query("DELETE FROM refresh_tokens WHERE session_id = ?")
        .bind(fixture.session_id(&login))
        .execute(&fixture.pool)
        .await
        .unwrap();
    fixture.assert_authorized(&login).await;
    fixture
        .service
        .logout_with_principal(&login.refresh_token, Some(&principal))
        .await
        .unwrap();
    fixture.assert_revoked(&login).await;
    fixture.assert_authorized(&other).await;
}
