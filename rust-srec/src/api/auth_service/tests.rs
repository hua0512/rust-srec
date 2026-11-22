use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use super::cache::CachedApiKeyState;
use super::*;
use crate::api::jwt::{Claims, JwtService};
use crate::database::models::{
    ApiKeyAccessLevel, AuthSessionDbModel, RefreshTokenDbModel, UserDbModel,
};
use crate::database::repositories::refresh_token::RefreshTokenRotation;
use crate::database::repositories::{RefreshTokenRepository, UserRepository};

#[test]
fn test_verify_password_legacy_argon2id_hash() {
    // Argon2 0.5's PHC test vector also checks that verification reads stored parameters.
    let hash = concat!(
        "$argon2id$v=19$m=65536,t=2,p=1$c29tZXNhbHQ$",
        "CTFhFdXPJO1aFaMaO6Mm5c8y7cJHAph8ArZWb2GRPPc"
    );

    assert!(AuthService::verify_password("password", hash).unwrap());
    assert!(!AuthService::verify_password("wrong-password", hash).unwrap());
}

struct MockRefreshTokenRepository;

#[async_trait::async_trait]
impl RefreshTokenRepository for MockRefreshTokenRepository {
    async fn create_session(
        &self,
        _session: &AuthSessionDbModel,
        _token: &RefreshTokenDbModel,
        _expected_password_hash: &str,
    ) -> crate::Result<bool> {
        Ok(true)
    }
    async fn find_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> crate::Result<Option<AuthSessionDbModel>> {
        Ok(Some(AuthSessionDbModel {
            id: session_id.to_owned(),
            user_id: user_id.to_owned(),
            created_at: 0,
            expires_at: i64::MAX,
            revoked_at: None,
        }))
    }
    async fn rotate(
        &self,
        _id: &str,
        _replacement: &RefreshTokenDbModel,
    ) -> crate::Result<RefreshTokenRotation> {
        Ok(RefreshTokenRotation::NotFound)
    }

    async fn create(&self, _token: &RefreshTokenDbModel) -> crate::Result<()> {
        Ok(())
    }
    async fn find_by_token_hash(&self, _hash: &str) -> crate::Result<Option<RefreshTokenDbModel>> {
        Ok(None)
    }
    async fn find_active_by_user(&self, _user_id: &str) -> crate::Result<Vec<RefreshTokenDbModel>> {
        Ok(vec![])
    }
    async fn revoke(&self, _id: &str) -> crate::Result<()> {
        Ok(())
    }
    async fn revoke_all_for_user(&self, _user_id: &str) -> crate::Result<()> {
        Ok(())
    }
    async fn count_active_by_user(&self, _user_id: &str) -> crate::Result<i64> {
        Ok(0)
    }
}

struct SpyUserRepository {
    user: UserDbModel,
}

#[async_trait::async_trait]
impl UserRepository for SpyUserRepository {
    async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
        Ok(())
    }
    async fn find_by_id(&self, id: &str) -> crate::Result<Option<UserDbModel>> {
        if self.user.id == id {
            Ok(Some(self.user.clone()))
        } else {
            Ok(None)
        }
    }
    async fn find_by_username(&self, _username: &str) -> crate::Result<Option<UserDbModel>> {
        Ok(None)
    }
    async fn find_by_email(&self, _email: &str) -> crate::Result<Option<UserDbModel>> {
        Ok(None)
    }
    async fn update(&self, _user: &UserDbModel) -> crate::Result<()> {
        Ok(())
    }
    async fn delete(&self, _id: &str) -> crate::Result<()> {
        Ok(())
    }
    async fn list(&self, _limit: i64, _offset: i64) -> crate::Result<Vec<UserDbModel>> {
        Ok(vec![])
    }
    async fn update_last_login(&self, _id: &str, _time_ms: i64) -> crate::Result<()> {
        Ok(())
    }
    async fn update_password(&self, _id: &str, _hash: &str, _clear: bool) -> crate::Result<()> {
        Ok(())
    }
    async fn count(&self) -> crate::Result<i64> {
        Ok(0)
    }
}

struct CountingUserRepository {
    user: std::sync::Mutex<UserDbModel>,
    find_by_id_calls: AtomicUsize,
    fail_find_by_id: AtomicBool,
}

impl CountingUserRepository {
    fn new(user: UserDbModel) -> Self {
        Self {
            user: std::sync::Mutex::new(user),
            find_by_id_calls: AtomicUsize::new(0),
            fail_find_by_id: AtomicBool::new(false),
        }
    }

    fn find_by_id_calls(&self) -> usize {
        self.find_by_id_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl UserRepository for CountingUserRepository {
    async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> crate::Result<Option<UserDbModel>> {
        self.find_by_id_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_find_by_id.load(Ordering::SeqCst) {
            return Err(crate::Error::Database("lookup failed".to_string()));
        }
        let user = self.user.lock().expect("user lock").clone();
        Ok((user.id == id).then_some(user))
    }

    async fn find_by_username(&self, _username: &str) -> crate::Result<Option<UserDbModel>> {
        Ok(None)
    }

    async fn find_by_email(&self, _email: &str) -> crate::Result<Option<UserDbModel>> {
        Ok(None)
    }

    async fn update(&self, _user: &UserDbModel) -> crate::Result<()> {
        Ok(())
    }

    async fn delete(&self, _id: &str) -> crate::Result<()> {
        Ok(())
    }

    async fn list(&self, _limit: i64, _offset: i64) -> crate::Result<Vec<UserDbModel>> {
        Ok(vec![])
    }

    async fn update_last_login(&self, _id: &str, _time_ms: i64) -> crate::Result<()> {
        Ok(())
    }

    async fn update_password(&self, id: &str, hash: &str, clear: bool) -> crate::Result<()> {
        let mut user = self.user.lock().expect("user lock");
        if user.id == id {
            user.password_hash = hash.to_string();
            if clear {
                user.must_change_password = false;
            }
        }
        Ok(())
    }

    async fn count(&self) -> crate::Result<i64> {
        Ok(0)
    }
}

fn create_counting_access_service(
    user: UserDbModel,
) -> (AuthService, Arc<CountingUserRepository>, Arc<JwtService>) {
    let repo = Arc::new(CountingUserRepository::new(user));
    let jwt_service = Arc::new(
        JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        )
        .unwrap(),
    );
    let service = AuthService::new(
        repo.clone(),
        Arc::new(MockRefreshTokenRepository),
        Arc::new(crate::database::repositories::InMemoryApiKeyRepository::default()),
        jwt_service.clone(),
        AuthConfig::default(),
    );
    (service, repo, jwt_service)
}

fn mint_access_token(secret: &str, user_id: &str, exp: u64, iat: u64) -> String {
    let claims = Claims {
        sub: user_id.to_string(),
        roles: vec!["user".to_string()],
        iss: "test-issuer".to_string(),
        aud: "test-audience".to_string(),
        exp,
        iat,
        sid: Some("test-session".to_owned()),
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()).expect("valid test JWT secret"),
    )
    .expect("token encoding should succeed")
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be past the Unix epoch")
        .as_secs()
}

#[tokio::test]
async fn authorize_access_token_fails_closed_with_fresh_permissive_cache() {
    let mut user = UserDbModel::new("stale-error", "hash", vec!["user".to_string()]);
    user.must_change_password = false;
    let user_id = user.id.clone();
    let (service, repo, jwt_service) = create_counting_access_service(user);
    let token = jwt_service
        .generate_test_token(&user_id, vec!["user".to_string()])
        .expect("token generation should succeed");

    service
        .authorize_access_token(&token, false)
        .await
        .expect("first authorize should succeed");
    repo.fail_find_by_id.store(true, Ordering::SeqCst);

    let result = service.authorize_access_token(&token, false).await;
    assert!(
        matches!(result, Err(AuthError::Database(_))),
        "a cached grant must not back an Ok decision when find_by_id fails"
    );
    assert_eq!(repo.find_by_id_calls(), 2);
}

#[tokio::test]
async fn authorize_access_token_rejects_expired_token_before_user_lookup() {
    let mut user = UserDbModel::new("expired-token", "hash", vec!["user".to_string()]);
    user.must_change_password = false;
    let user_id = user.id.clone();
    let (service, repo, _jwt_service) = create_counting_access_service(user);
    let now = unix_now_secs();
    let token = mint_access_token(
        "test-secret-key-32-chars-long!!",
        &user_id,
        now - 3600,
        now - 7200,
    );

    let result = service.authorize_access_token(&token, false).await;
    assert!(matches!(result, Err(AuthError::TokenExpired)));
    assert_eq!(
        repo.find_by_id_calls(),
        0,
        "token validation failures must be decided before find_by_id"
    );
}

#[tokio::test]
async fn api_key_revocation_ignores_a_stale_cache_fill() {
    let mut user = UserDbModel::new("u", "hash", vec!["admin".to_string()]);
    user.must_change_password = false;
    let user_id = user.id.clone();
    let user_repo = Arc::new(SpyUserRepository { user });
    let jwt_service = Arc::new(
        JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        )
        .unwrap(),
    );
    let service = AuthService::new(
        user_repo,
        Arc::new(MockRefreshTokenRepository),
        Arc::new(crate::database::repositories::InMemoryApiKeyRepository::default()),
        jwt_service,
        AuthConfig::default(),
    );

    let (model, raw_key) = service
        .create_api_key(&user_id, "racing key", ApiKeyAccessLevel::Full, None)
        .await
        .expect("key creation should succeed");
    let stale_generation = service.api_key_cache_generation.load(Ordering::Acquire);
    let revoked = service
        .revoke_api_key(&user_id, &model.id)
        .await
        .expect("revoke should succeed");
    assert!(revoked);

    let key_hash = AuthService::hash_token(&raw_key);
    service.api_key_state_cache.insert(
        key_hash,
        CachedApiKeyState {
            generation: stale_generation,
            fetched_at: Instant::now(),
            key_id: model.id,
            user_id,
            roles: vec!["admin".to_string()],
            user_is_active: true,
            must_change_password: false,
            access_level: ApiKeyAccessLevel::Full,
            expires_at: None,
            revoked: false,
        },
    );

    let result = service.authorize_api_key(&raw_key).await;
    assert!(matches!(result, Err(AuthError::TokenRevoked)));
}
