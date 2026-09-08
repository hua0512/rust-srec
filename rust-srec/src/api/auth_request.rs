//! Shared authentication for routes outside the standard HTTP middleware.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};

use crate::api::auth_service::{AuthPrincipal, AuthService};
use crate::api::error::ApiError;
use crate::database::models::ApiKeyAccessLevel;

#[derive(Clone, Copy)]
pub(crate) enum AccessPolicy {
    Read,
    Full,
}

/// An explicit header is authoritative, including when malformed. Query tokens
/// are only a transport fallback for clients that cannot send that header.
pub(crate) fn request_credential<'a>(
    headers: &'a HeaderMap,
    query: Option<&'a str>,
) -> Result<&'a str, ApiError> {
    let token = if let Some(header) = headers.get(AUTHORIZATION) {
        if headers.get_all(AUTHORIZATION).iter().count() != 1 {
            return Err(ApiError::unauthorized(
                "Multiple Authorization headers are not supported",
            ));
        }
        header
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("Invalid Authorization header"))?
    } else {
        query.ok_or_else(|| ApiError::unauthorized("Missing authorization credential"))?
    };
    if token.is_empty() {
        return Err(ApiError::unauthorized("Missing authorization credential"));
    }
    Ok(token)
}

pub(crate) fn enforce_access(
    principal: &AuthPrincipal,
    policy: AccessPolicy,
) -> Result<(), ApiError> {
    if matches!(policy, AccessPolicy::Full) && principal.access != ApiKeyAccessLevel::Full {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "API_KEY_READ_ONLY",
            "This endpoint requires a full-access API key",
        ));
    }
    Ok(())
}

pub(crate) async fn authorize_request(
    service: Option<&Arc<AuthService>>,
    headers: &HeaderMap,
    query: Option<&str>,
    policy: AccessPolicy,
) -> Result<Option<AuthPrincipal>, ApiError> {
    let Some(service) = service else {
        return Ok(None);
    };
    let principal = service
        .authorize_credential(request_credential(headers, query)?, false)
        .await
        .map_err(ApiError::from)?;
    enforce_access(&principal, policy)?;
    Ok(Some(principal))
}

pub(crate) async fn revalidate(
    service: Option<&Arc<AuthService>>,
    principal: Option<&AuthPrincipal>,
    policy: AccessPolicy,
) -> Result<(), ApiError> {
    let Some(service) = service else {
        return Ok(());
    };
    let principal =
        principal.ok_or_else(|| ApiError::unauthorized("Credential identity is missing"))?;
    let current = service
        .revalidate_principal(principal)
        .await
        .map_err(ApiError::from)?;
    enforce_access(&current, policy)
}

/// Own the whole socket handler while revalidating. Revocation drops even a
/// handler blocked in a send, so backpressure cannot postpone disconnection.
pub(crate) async fn run_authenticated_session(
    service: Option<Arc<AuthService>>,
    principal: Option<AuthPrincipal>,
    policy: AccessPolicy,
    session: impl Future<Output = ()>,
) {
    if service.is_none() {
        session.await;
        return;
    }
    if principal.is_none() {
        return;
    }
    run_guarded_session(session, move || {
        let service = service.clone();
        let principal = principal.clone();
        async move {
            revalidate(service.as_ref(), principal.as_ref(), policy)
                .await
                .map_err(|_| ())
        }
    })
    .await;
}

async fn run_guarded_session<F, C>(session: impl Future<Output = ()>, mut check: C)
where
    C: FnMut() -> F,
    F: Future<Output = Result<(), ()>>,
{
    let validation = async {
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(5),
            Duration::from_secs(5),
        );
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if !matches!(
                tokio::time::timeout(Duration::from_secs(3), check()).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    };
    tokio::select! {
        biased;
        _ = validation => {},
        _ = session => {},
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) async fn open_socket(
        address: std::net::SocketAddr,
        path: &str,
        authorization: Option<&str>,
        expected_status: u16,
    ) -> tokio::net::TcpStream {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            let auth = authorization.map(|value| format!("Authorization: {value}\r\n")).unwrap_or_default();
            socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{auth}\r\n").as_bytes()).await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                assert!(headers.len() < 16384, "response headers must be bounded");
                headers.push(socket.read_u8().await.unwrap());
            }
            let headers = String::from_utf8(headers).unwrap();
            assert!(headers.starts_with(&format!("HTTP/1.1 {expected_status} ")), "{headers}");
            socket
        }).await.expect("socket handshake must complete")
    }

    pub(crate) async fn wait_for_socket_close(mut socket: tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(Duration::from_secs(9), async {
            let mut buffer = [0; 4096];
            loop {
                match socket.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        break;
                    }
                    Err(error) => panic!("socket read failed: {error}"),
                }
            }
        })
        .await
        .expect("revoked socket must close within the revalidation deadline");
    }

    pub(crate) struct AuthFixture {
        pub pool: sqlx::SqlitePool,
        pub service: Arc<AuthService>,
        pub user_id: String,
        pub access_token: String,
        pub refresh_token: String,
    }

    pub(crate) async fn fixture() -> AuthFixture {
        use crate::api::{auth_service::AuthConfig, jwt::JwtService};
        use crate::database::{
            models::UserDbModel,
            repositories::{
                SqlxApiKeyRepository, SqlxRefreshTokenRepository, SqlxUserRepository,
                UserRepository,
            },
        };
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let users = Arc::new(SqlxUserRepository::new(pool.clone(), pool.clone()));
        let hash = tokio::task::spawn_blocking(|| AuthService::hash_password("fixture-password"))
            .await
            .unwrap()
            .unwrap();
        let mut user = UserDbModel::new("manual-route-user", hash, vec!["user".to_owned()]);
        user.must_change_password = false;
        users.create(&user).await.unwrap();
        let service = Arc::new(AuthService::new(
            users,
            Arc::new(SqlxRefreshTokenRepository::new(pool.clone(), pool.clone())),
            Arc::new(SqlxApiKeyRepository::new(pool.clone(), pool.clone())),
            Arc::new(JwtService::new(
                "manual-route-secret-long-enough",
                "fixture",
                "fixture",
                Some(3600),
            )),
            AuthConfig::default(),
        ));
        let login = service
            .authenticate("manual-route-user", "fixture-password", None, None)
            .await
            .unwrap();
        AuthFixture {
            pool,
            service,
            user_id: user.id,
            access_token: login.access_token,
            refresh_token: login.refresh_token,
        }
    }

    #[tokio::test]
    async fn manual_auth_accepts_keys_enforces_scope_and_revalidates_current_identity() {
        let fixture = fixture().await;
        let (key, raw) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "manual-read",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let headers = HeaderMap::new();
        let read = authorize_request(
            Some(&fixture.service),
            &headers,
            Some(&raw),
            AccessPolicy::Read,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(read.api_key_id.as_deref(), Some(key.id.as_str()));
        assert_eq!(
            authorize_request(
                Some(&fixture.service),
                &headers,
                Some(&raw),
                AccessPolicy::Full
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::FORBIDDEN
        );
        assert!(
            fixture
                .service
                .revoke_api_key(&fixture.user_id, &key.id)
                .await
                .unwrap()
        );
        assert!(
            revalidate(Some(&fixture.service), Some(&read), AccessPolicy::Read)
                .await
                .is_err()
        );

        let (key, raw) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "manual-full",
                ApiKeyAccessLevel::Full,
                None,
            )
            .await
            .unwrap();
        let full = authorize_request(
            Some(&fixture.service),
            &headers,
            Some(&raw),
            AccessPolicy::Full,
        )
        .await
        .unwrap()
        .unwrap();
        sqlx::query("UPDATE api_keys SET access_level = ? WHERE id = ?")
            .bind(ApiKeyAccessLevel::ReadOnly.as_str())
            .bind(&key.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(
            revalidate(Some(&fixture.service), Some(&full), AccessPolicy::Full)
                .await
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );

        let principal = authorize_request(
            Some(&fixture.service),
            &headers,
            Some(&fixture.access_token),
            AccessPolicy::Full,
        )
        .await
        .unwrap()
        .unwrap();
        fixture
            .service
            .logout(&fixture.refresh_token)
            .await
            .unwrap();
        assert!(
            revalidate(Some(&fixture.service), Some(&principal), AccessPolicy::Full)
                .await
                .is_err()
        );
        fixture.pool.close().await;
    }

    #[test]
    fn authorization_header_is_authoritative_over_query_fallback() {
        let mut headers = HeaderMap::new();
        assert_eq!(
            request_credential(&headers, Some("query")).unwrap(),
            "query"
        );
        headers.insert(AUTHORIZATION, "Bearer header".parse().unwrap());
        assert_eq!(
            request_credential(&headers, Some("query")).unwrap(),
            "header"
        );
        for value in ["Basic invalid", "Bearer "] {
            headers.insert(AUTHORIZATION, value.parse().unwrap());
            assert!(request_credential(&headers, Some("valid-query")).is_err());
        }
        headers.insert(
            AUTHORIZATION,
            axum::http::HeaderValue::from_bytes(b"Bearer \xff").unwrap(),
        );
        assert!(request_credential(&headers, Some("valid-query")).is_err());
        headers.insert(AUTHORIZATION, "Bearer first".parse().unwrap());
        headers.append(AUTHORIZATION, "Bearer second".parse().unwrap());
        assert!(request_credential(&headers, Some("valid-query")).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn socket_revalidation_interrupts_blocked_handlers_and_bounds_stalled_checks() {
        for stalled in [false, true] {
            let (dropped, observed) = tokio::sync::oneshot::channel();
            struct SessionDrop(Option<tokio::sync::oneshot::Sender<()>>);
            impl Drop for SessionDrop {
                fn drop(&mut self) {
                    if let Some(sender) = self.0.take() {
                        let _ = sender.send(());
                    }
                }
            }
            let guard = SessionDrop(Some(dropped));
            let session = async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            };
            let started = tokio::time::Instant::now();
            run_guarded_session(session, move || async move {
                if stalled {
                    std::future::pending::<()>().await;
                }
                Err(())
            })
            .await;
            assert_eq!(
                started.elapsed(),
                Duration::from_secs(if stalled { 8 } else { 5 })
            );
            observed.await.unwrap();
        }
    }
    #[tokio::test]
    async fn disabled_user_cannot_use_cached_session_or_revalidated_key() {
        let fixture = fixture().await;
        let (_, raw) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "disabled-full",
                ApiKeyAccessLevel::Full,
                None,
            )
            .await
            .unwrap();
        let headers = HeaderMap::new();
        let key = authorize_request(
            Some(&fixture.service),
            &headers,
            Some(&raw),
            AccessPolicy::Full,
        )
        .await
        .unwrap()
        .unwrap();
        let session = authorize_request(
            Some(&fixture.service),
            &headers,
            Some(&fixture.access_token),
            AccessPolicy::Full,
        )
        .await
        .unwrap()
        .unwrap();
        sqlx::query("UPDATE users SET is_active = 0 WHERE id = ?")
            .bind(&fixture.user_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert!(
            authorize_request(
                Some(&fixture.service),
                &headers,
                Some(&fixture.access_token),
                AccessPolicy::Full
            )
            .await
            .is_err()
        );
        assert!(
            revalidate(Some(&fixture.service), Some(&session), AccessPolicy::Full)
                .await
                .is_err()
        );
        assert!(
            revalidate(Some(&fixture.service), Some(&key), AccessPolicy::Full)
                .await
                .is_err()
        );
        fixture.pool.close().await;
    }
}
