use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Barrier;

use super::{AuthConfig, AuthError, AuthService};
use crate::api::jwt::JwtService;
use crate::database;
use crate::database::models::{RefreshTokenDbModel, UserDbModel};
use crate::database::repositories::refresh_token::RefreshTokenRotation;
use crate::database::repositories::{
    RefreshTokenRepository, SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
    UserRepository,
};

const RAW_TOKEN: &str = "test-rotation-predecessor";

fn token_hash(raw: &str) -> String {
    AuthService::hash_token(raw)
}

fn token(user_id: &str, raw: &str) -> RefreshTokenDbModel {
    RefreshTokenDbModel::new(
        user_id,
        token_hash(raw),
        Utc::now() + chrono::Duration::days(7),
        Some("test-device".to_string()),
    )
}

/// Both requests read the active predecessor before either is allowed to rotate it.
struct ConcurrentReaders {
    inner: Arc<SqlxRefreshTokenRepository>,
    barrier: Barrier,
}

#[async_trait]
impl RefreshTokenRepository for ConcurrentReaders {
    async fn create(&self, token: &RefreshTokenDbModel) -> crate::Result<()> {
        self.inner.create(token).await
    }

    async fn rotate(
        &self,
        id: &str,
        replacement: &RefreshTokenDbModel,
    ) -> crate::Result<RefreshTokenRotation> {
        self.inner.rotate(id, replacement).await
    }

    async fn find_by_token_hash(&self, hash: &str) -> crate::Result<Option<RefreshTokenDbModel>> {
        let token = self.inner.find_by_token_hash(hash).await?;
        assert!(token.as_ref().is_some_and(|token| !token.is_revoked()));
        self.barrier.wait().await;
        Ok(token)
    }

    async fn find_active_by_user(&self, user_id: &str) -> crate::Result<Vec<RefreshTokenDbModel>> {
        self.inner.find_active_by_user(user_id).await
    }

    async fn revoke(&self, id: &str) -> crate::Result<()> {
        self.inner.revoke(id).await
    }

    async fn revoke_all_for_user(&self, user_id: &str) -> crate::Result<()> {
        self.inner.revoke_all_for_user(user_id).await
    }

    async fn count_active_by_user(&self, user_id: &str) -> crate::Result<i64> {
        self.inner.count_active_by_user(user_id).await
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    pool: sqlx::SqlitePool,
    tokens: Arc<SqlxRefreshTokenRepository>,
    service: AuthService,
    user: UserDbModel,
    predecessor: RefreshTokenDbModel,
}

impl Fixture {
    async fn new(config: AuthConfig, concurrent: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite:{}?mode=rwc",
            dir.path()
                .join("auth.db")
                .to_string_lossy()
                .replace('\\', "/")
        );
        let pool = database::init_pool_with_size(&url, 4).await.unwrap();
        database::run_migrations(&pool).await.unwrap();
        let users = Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone()));
        let user = UserDbModel::new(
            "rotation-user",
            "unused-password-hash",
            vec!["user".to_string()],
        );
        users.create(&user).await.unwrap();
        let tokens = Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone()));
        let predecessor = token(&user.id, RAW_TOKEN);
        tokens.create(&predecessor).await.unwrap();
        let repository: Arc<dyn RefreshTokenRepository> = if concurrent {
            Arc::new(ConcurrentReaders {
                inner: tokens.clone(),
                barrier: Barrier::new(2),
            })
        } else {
            tokens.clone()
        };
        let service = AuthService::new(
            users,
            repository,
            Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool.clone())),
            Arc::new(JwtService::new(
                "test-secret-key-32-chars-long!!",
                "test",
                "test",
                Some(900),
            )),
            config,
        );
        Self {
            _dir: dir,
            pool,
            tokens,
            service,
            user,
            predecessor,
        }
    }

    async fn predecessor(&self) -> RefreshTokenDbModel {
        self.tokens
            .find_by_token_hash(&token_hash(RAW_TOKEN))
            .await
            .unwrap()
            .unwrap()
    }

    async fn total_tokens(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM refresh_tokens WHERE user_id = ?")
            .bind(&self.user.id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn concurrent_refresh_has_one_successor_and_applies_configured_reuse_policy() {
    for (config, expected_active) in [
        (AuthConfig::default(), 0),
        (
            AuthConfig {
                revoke_all_on_refresh_token_reuse: false,
                ..AuthConfig::default()
            },
            2,
        ),
        (
            AuthConfig {
                refresh_token_reuse_grace_secs: 60,
                ..AuthConfig::default()
            },
            2,
        ),
    ] {
        let fixture = Fixture::new(config, true).await;
        fixture
            .tokens
            .create(&token(&fixture.user.id, "other-session"))
            .await
            .unwrap();
        let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                fixture.service.refresh_tokens(RAW_TOKEN),
                fixture.service.refresh_tokens(RAW_TOKEN)
            )
        })
        .await
        .expect("concurrent refreshes must settle");
        let (winner, loser) = match first {
            Ok(winner) => (winner, second),
            Err(error) => (second.unwrap(), Err(error)),
        };
        assert!(matches!(loser, Err(AuthError::TokenRevoked)));
        assert_eq!(
            fixture.total_tokens().await,
            3,
            "one predecessor, one other session, one successor"
        );
        assert_eq!(
            fixture
                .tokens
                .count_active_by_user(&fixture.user.id)
                .await
                .unwrap(),
            expected_active
        );
        let successor = fixture
            .tokens
            .find_by_token_hash(&token_hash(&winner.refresh_token))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(successor.device_info.as_deref(), Some("test-device"));
        assert_eq!(successor.is_revoked(), expected_active == 0);
        assert!(fixture.predecessor().await.is_revoked());
        fixture.pool.close().await;
    }
}

#[tokio::test]
async fn sequential_refresh_reuse_revokes_successor_by_default() {
    let fixture = Fixture::new(AuthConfig::default(), false).await;
    let first = fixture.service.refresh_tokens(RAW_TOKEN).await.unwrap();
    assert!(matches!(
        fixture.service.refresh_tokens(RAW_TOKEN).await,
        Err(AuthError::TokenRevoked)
    ));
    assert!(
        fixture
            .tokens
            .find_by_token_hash(&token_hash(&first.refresh_token))
            .await
            .unwrap()
            .unwrap()
            .is_revoked()
    );
    assert_eq!(fixture.total_tokens().await, 2);
    fixture.pool.close().await;
}

#[tokio::test]
async fn grace_replay_never_reissues_or_extends_the_original_revocation() {
    let fixture = Fixture::new(
        AuthConfig {
            refresh_token_reuse_grace_secs: 60,
            ..AuthConfig::default()
        },
        false,
    )
    .await;
    fixture.service.refresh_tokens(RAW_TOKEN).await.unwrap();
    let original = database::time::now_ms() - 30_000;
    sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE id = ?")
        .bind(original)
        .bind(&fixture.predecessor.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            fixture.service.refresh_tokens(RAW_TOKEN).await,
            Err(AuthError::TokenRevoked)
        ));
    }
    fixture
        .tokens
        .revoke(&fixture.predecessor.id)
        .await
        .unwrap();
    assert_eq!(fixture.predecessor().await.revoked_at, Some(original));
    assert_eq!(fixture.total_tokens().await, 2);
    assert_eq!(
        fixture
            .tokens
            .count_active_by_user(&fixture.user.id)
            .await
            .unwrap(),
        1
    );
    sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE id = ?")
        .bind(database::time::now_ms() - 61_000)
        .bind(&fixture.predecessor.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert!(matches!(
        fixture.service.refresh_tokens(RAW_TOKEN).await,
        Err(AuthError::TokenRevoked)
    ));
    assert_eq!(
        fixture
            .tokens
            .count_active_by_user(&fixture.user.id)
            .await
            .unwrap(),
        0
    );
    fixture.pool.close().await;
}

#[tokio::test]
async fn grace_cannot_refresh_a_logged_out_token() {
    let fixture = Fixture::new(
        AuthConfig {
            refresh_token_reuse_grace_secs: 60,
            ..AuthConfig::default()
        },
        false,
    )
    .await;
    fixture.service.logout(RAW_TOKEN).await.unwrap();
    let revoked_at = fixture.predecessor().await.revoked_at;
    assert!(matches!(
        fixture.service.refresh_tokens(RAW_TOKEN).await,
        Err(AuthError::TokenRevoked)
    ));
    assert_eq!(fixture.total_tokens().await, 1);
    assert_eq!(fixture.predecessor().await.revoked_at, revoked_at);
    fixture.pool.close().await;
}

#[tokio::test]
async fn replacement_insert_failure_rolls_back_predecessor_consumption() {
    let fixture = Fixture::new(AuthConfig::default(), false).await;
    sqlx::query("CREATE TRIGGER reject_replacement BEFORE INSERT ON refresh_tokens BEGIN SELECT RAISE(FAIL, 'injected insert failure'); END")
        .execute(&fixture.pool).await.unwrap();
    assert!(matches!(
        fixture.service.refresh_tokens(RAW_TOKEN).await,
        Err(AuthError::Database(_))
    ));
    assert!(!fixture.predecessor().await.is_revoked());
    assert_eq!(fixture.total_tokens().await, 1);
    sqlx::query("DROP TRIGGER reject_replacement")
        .execute(&fixture.pool)
        .await
        .unwrap();
    fixture.service.refresh_tokens(RAW_TOKEN).await.unwrap();
    assert!(fixture.predecessor().await.is_revoked());
    assert_eq!(fixture.total_tokens().await, 2);
    fixture.pool.close().await;
}

#[tokio::test]
async fn rotation_rechecks_expiry_and_revocation_against_database_state() {
    let fixture = Fixture::new(AuthConfig::default(), false).await;
    let replacement = token(&fixture.user.id, "replacement");
    sqlx::query("UPDATE refresh_tokens SET expires_at = ? WHERE id = ?")
        .bind(database::time::now_ms() - 1)
        .bind(&fixture.predecessor.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .tokens
            .rotate(&fixture.predecessor.id, &replacement)
            .await
            .unwrap(),
        RefreshTokenRotation::Expired
    );
    fixture
        .tokens
        .revoke_all_for_user(&fixture.user.id)
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .tokens
            .rotate(&fixture.predecessor.id, &replacement)
            .await
            .unwrap(),
        RefreshTokenRotation::Revoked { .. }
    ));
    assert_eq!(
        fixture
            .tokens
            .rotate("missing", &replacement)
            .await
            .unwrap(),
        RefreshTokenRotation::NotFound
    );
    assert_eq!(fixture.total_tokens().await, 1);
    fixture.pool.close().await;
}

#[tokio::test]
async fn disabled_account_does_not_consume_refresh_token() {
    let fixture = Fixture::new(AuthConfig::default(), false).await;
    sqlx::query("UPDATE users SET is_active = 0 WHERE id = ?")
        .bind(&fixture.user.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert!(matches!(
        fixture.service.refresh_tokens(RAW_TOKEN).await,
        Err(AuthError::AccountDisabled)
    ));
    assert!(!fixture.predecessor().await.is_revoked());
    assert_eq!(fixture.total_tokens().await, 1);
    fixture.pool.close().await;
}
