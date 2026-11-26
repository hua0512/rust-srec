//! Shared authentication for routes outside the standard HTTP middleware.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};

use crate::api::auth_service::{AuthPrincipal, AuthService};
use crate::api::auth_store::ApiKeyAccessLevel;
use crate::api::error::ApiError;

/// How often an open socket rechecks the credential it was opened with.
const SOCKET_REVALIDATION_INTERVAL: Duration = Duration::from_secs(5);

/// Replaces [`SOCKET_REVALIDATION_INTERVAL`] when set.
static SOCKET_REVALIDATION_OVERRIDE: parking_lot::RwLock<Option<Duration>> =
    parking_lot::RwLock::new(None);

/// Recheck socket credentials every `interval` instead of every 5 seconds, so
/// end-to-end tests can observe a revoked socket closing without waiting for
/// the production cadence. Applies to sockets opened afterwards.
#[doc(hidden)]
pub fn override_socket_revalidation_interval(interval: Duration) {
    *SOCKET_REVALIDATION_OVERRIDE.write() = Some(interval);
}

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
        .await?;
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
    let current = service.revalidate_principal(principal).await?;
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
    let interval = SOCKET_REVALIDATION_OVERRIDE
        .read()
        .unwrap_or(SOCKET_REVALIDATION_INTERVAL);
    let validation = async {
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
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
mod tests {
    use super::*;

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
}
