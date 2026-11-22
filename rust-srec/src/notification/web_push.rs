use crate::database::models::WebPushSubscriptionDbModel;
use crate::database::{DbPool, WritePool};
use crate::notification::events::{NotificationEvent, NotificationPriority};
use crate::{Error, Result};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit};
use base64::Engine as _;
use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use hkdf::Hkdf;
use mesio::redact::redact_url_for_log;
use p256::ecdh::EphemeralSecret;
use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use rand::Rng;
use serde::Serialize;
use sha2::Sha256;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use std::time::Instant;
use tokio::net::lookup_host;
use url::Url;

use crate::proxies::{ProxyRoute, ProxyTarget, ServiceRoutes, SystemProxy};
use crate::utils::http_client::{ClientCache, apply_proxy, build_client, install_rustls_provider};

const SALT_LEN: usize = 16;
const PUBLIC_KEY_LEN: usize = 65;
const AUTH_SECRET_LEN: usize = 16;
const DEFAULT_RS: u32 = 4096;
const DEFAULT_CONCURRENCY: usize = 16;
const MAX_PAYLOAD_BYTES: usize = 3500;
const SUBSCRIPTION_CACHE_TTL: Duration = Duration::from_secs(30);
const VAPID_JWT_EXP_SECS: i64 = 12 * 60 * 60;
const VAPID_JWT_SKEW_SECS: i64 = 60;
const PUSH_TIMEOUT: Duration = Duration::from_secs(15);

type SubscriptionCacheValue = Option<(Instant, Vec<WebPushSubscriptionDbModel>)>;
type SubscriptionCache = std::sync::Arc<tokio::sync::RwLock<SubscriptionCacheValue>>;

#[derive(Clone)]
struct CachedVapidJwt {
    jwt: String,
    exp_unix: i64,
}

impl std::fmt::Debug for CachedVapidJwt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedVapidJwt")
            .field("jwt", &"[REDACTED]")
            .field("exp_unix", &self.exp_unix)
            .finish()
    }
}

const IKM_INFO_PREFIX: &str = "WebPush: info\0";
const KEY_INFO: &str = "Content-Encoding: aes128gcm\0";
const NONCE_INFO: &str = "Content-Encoding: nonce\0";

#[cfg(test)]
mod delivery_tests;
#[cfg(test)]
mod redaction_tests;

#[derive(Clone)]
pub struct WebPushConfig {
    vapid_public_key_b64: String,
    vapid_private_key_raw: [u8; 32],
    vapid_subject: String,
}

impl std::fmt::Debug for WebPushConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebPushConfig")
            .field("vapid_public_key_b64", &self.vapid_public_key_b64)
            .field("vapid_private_key_raw", &"[REDACTED]")
            .field("vapid_subject", &"[REDACTED]")
            .finish()
    }
}

impl WebPushConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let vapid_public_key_b64 = std::env::var("WEB_PUSH_VAPID_PUBLIC_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let vapid_private_key_b64 = std::env::var("WEB_PUSH_VAPID_PRIVATE_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let (vapid_public_key_b64, vapid_private_key_b64) =
            match (vapid_public_key_b64, vapid_private_key_b64) {
                (None, None) => return Ok(None),
                (Some(public), Some(private)) => (public, private),
                _ => {
                    return Err(Error::config(
                        "Both WEB_PUSH_VAPID_PUBLIC_KEY and WEB_PUSH_VAPID_PRIVATE_KEY must be set"
                            .to_string(),
                    ));
                }
            };
        let vapid_subject = std::env::var("WEB_PUSH_VAPID_SUBJECT")
            .or_else(|_| std::env::var("WEB_PUSH_SUBJECT"))
            .unwrap_or_else(|_| "mailto:admin@localhost".to_string());

        let public_raw = decode_b64url(&vapid_public_key_b64)
            .map_err(|e| Error::config(format!("Invalid WEB_PUSH_VAPID_PUBLIC_KEY: {}", e)))?;
        let private_raw = decode_b64url(&vapid_private_key_b64)
            .map_err(|e| Error::config(format!("Invalid WEB_PUSH_VAPID_PRIVATE_KEY: {}", e)))?;

        let _public_raw: [u8; PUBLIC_KEY_LEN] = public_raw.try_into().map_err(|_| {
            Error::config(format!(
                "WEB_PUSH_VAPID_PUBLIC_KEY must decode to {} bytes",
                PUBLIC_KEY_LEN
            ))
        })?;
        let private_raw: [u8; 32] = private_raw.try_into().map_err(|_| {
            Error::config("WEB_PUSH_VAPID_PRIVATE_KEY must decode to 32 bytes".to_string())
        })?;

        Ok(Some(Self {
            vapid_public_key_b64,
            vapid_private_key_raw: private_raw,
            vapid_subject,
        }))
    }

    pub fn vapid_public_key_b64(&self) -> &str {
        &self.vapid_public_key_b64
    }
}

#[derive(Clone)]
pub struct WebPushService {
    pool: DbPool,
    write_pool: WritePool,
    config: WebPushConfig,
    subscription_cache: SubscriptionCache,
    vapid_jwt_cache: DashMap<String, CachedVapidJwt>,
    metrics: std::sync::Arc<
        parking_lot::RwLock<Option<std::sync::Arc<crate::metrics::MetricsCollector>>>,
    >,
}

impl std::fmt::Debug for WebPushService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Cache keys/endpoints and subscription auth values are bearer credentials too.
        // Do not inspect locked cache contents while formatting diagnostics.
        f.debug_struct("WebPushService")
            .field("config", &self.config)
            .field("subscription_cache", &"[REDACTED]")
            .field("vapid_jwt_cache", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl WebPushService {
    pub fn new(pool: DbPool, write_pool: WritePool, config: WebPushConfig) -> Result<Self> {
        crate::utils::http_client::install_rustls_provider();
        Ok(Self {
            pool,
            write_pool,
            config,
            subscription_cache: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            vapid_jwt_cache: DashMap::new(),
            metrics: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        pool: DbPool,
        write_pool: WritePool,
        vapid_public_key_b64: String,
        vapid_private_key_raw: [u8; 32],
    ) -> Result<Self> {
        Self::new(
            pool,
            write_pool,
            WebPushConfig {
                vapid_public_key_b64,
                vapid_private_key_raw,
                vapid_subject: "mailto:test@localhost".into(),
            },
        )
    }

    pub fn from_env(pool: DbPool, write_pool: WritePool) -> Result<Option<Self>> {
        let Some(config) = WebPushConfig::from_env()? else {
            return Ok(None);
        };
        Ok(Some(Self::new(pool, write_pool, config)?))
    }

    pub fn vapid_public_key(&self) -> &str {
        self.config.vapid_public_key_b64()
    }

    pub fn set_metrics_collector(&self, metrics: std::sync::Arc<crate::metrics::MetricsCollector>) {
        *self.metrics.write() = Some(metrics);
    }

    pub async fn upsert_subscription(
        &self,
        user_id: &str,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        min_priority: NotificationPriority,
    ) -> Result<WebPushSubscriptionDbModel> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = jiff::Timestamp::now().as_millisecond();
        let min_priority = min_priority.as_int() as i32;

        sqlx::query(
            r#"
            INSERT INTO web_push_subscription (
                id, user_id, endpoint, p256dh, auth, min_priority, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(endpoint) DO UPDATE SET
                user_id = excluded.user_id,
                p256dh = excluded.p256dh,
                auth = excluded.auth,
                min_priority = excluded.min_priority,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(endpoint)
        .bind(p256dh)
        .bind(auth)
        .bind(min_priority)
        .bind(now)
        .bind(now)
        .execute(&self.write_pool)
        .await?;

        *self.subscription_cache.write().await = None;

        let row = sqlx::query_as::<_, WebPushSubscriptionDbModel>(
            "SELECT * FROM web_push_subscription WHERE endpoint = ? LIMIT 1",
        )
        .bind(endpoint)
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn unsubscribe(&self, user_id: &str, endpoint: &str) -> Result<()> {
        sqlx::query("DELETE FROM web_push_subscription WHERE user_id = ? AND endpoint = ?")
            .bind(user_id)
            .bind(endpoint)
            .execute(&self.write_pool)
            .await?;
        *self.subscription_cache.write().await = None;
        Ok(())
    }

    pub async fn list_subscriptions_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<WebPushSubscriptionDbModel>> {
        let rows = sqlx::query_as::<_, WebPushSubscriptionDbModel>(
            "SELECT * FROM web_push_subscription WHERE user_id = ? ORDER BY updated_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_all_subscriptions(&self) -> Result<Vec<WebPushSubscriptionDbModel>> {
        let rows = sqlx::query_as::<_, WebPushSubscriptionDbModel>(
            "SELECT * FROM web_push_subscription ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_all_subscriptions_cached(&self) -> Result<Vec<WebPushSubscriptionDbModel>> {
        if let Some((ts, cached)) = self.subscription_cache.read().await.as_ref()
            && ts.elapsed() < SUBSCRIPTION_CACHE_TTL
        {
            return Ok(cached.clone());
        }

        let rows = self.list_all_subscriptions().await?;
        *self.subscription_cache.write().await = Some((Instant::now(), rows.clone()));
        Ok(rows)
    }

    async fn delete_subscription_by_endpoint(&self, endpoint: &str) -> Result<()> {
        sqlx::query("DELETE FROM web_push_subscription WHERE endpoint = ?")
            .bind(endpoint)
            .execute(&self.write_pool)
            .await?;
        *self.subscription_cache.write().await = None;
        Ok(())
    }

    async fn allow_private_targets(&self) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT stream_proxy_allow_private_targets FROM global_config LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    }

    async fn route_target(&self) -> Result<ProxyTarget> {
        ServiceRoutes::new(self.pool.clone())
            .target(&ProxyRoute::Inherit)
            .await
    }

    /// Checks a subscription endpoint against the target policy that delivery
    /// applies. The hostname is resolved here unless delivery goes through a
    /// saved proxy: that proxy resolves it, and this host may have no usable
    /// DNS for the push service at all.
    pub async fn validate_endpoint(&self, endpoint: &str) -> Result<Url> {
        let allow_private = self.allow_private_targets().await;
        // Delivery fails on a route that cannot be resolved, so such an
        // endpoint gets the full check.
        let check_dns = match self.route_target().await {
            Ok(target) => may_dial_directly(&target),
            Err(_) => true,
        };
        validate_push_endpoint(endpoint, allow_private, check_dns).await
    }

    /// A non-redirecting HTTP client on the global route as it is now. A route
    /// naming a saved proxy that no longer exists is an error, never a direct
    /// connection.
    #[cfg(test)]
    async fn client(&self) -> Result<reqwest::Client> {
        let allow_private = self.allow_private_targets().await;
        let target = self.route_target().await?;
        web_push_client(&target, allow_private)
    }

    pub async fn send_event(&self, event: &NotificationEvent, event_log_id: Option<&str>) {
        let subscriptions = match self.list_all_subscriptions_cached().await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load web push subscriptions");
                return;
            }
        };

        if subscriptions.is_empty() {
            return;
        }

        let allow_private = self.allow_private_targets().await;
        let target = match self.route_target().await {
            Ok(target) => target,
            Err(error) => {
                tracing::warn!(%error, "Web push delivery skipped: no connection on the proxy route");
                return;
            }
        };
        let client = match web_push_client(&target, allow_private) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "Web push delivery skipped: HTTP client unavailable");
                return;
            }
        };
        let check_dns = may_dial_directly(&target);

        let now_ms = jiff::Timestamp::now().as_millisecond();
        let priority = event.priority();
        let event_log_id = event_log_id.map(|s| s.to_string());

        stream::iter(subscriptions)
            .for_each_concurrent(DEFAULT_CONCURRENCY, |sub| {
                let event_log_id = event_log_id.clone();
                let client = &client;
                async move {
                    if let Some(next_attempt_at_ms) = sub.next_attempt_at
                        && next_attempt_at_ms > now_ms
                    {
                        if let Some(metrics) = self.metrics.read().as_ref().cloned() {
                            metrics.record_web_push_skipped_backoff();
                        }
                        return;
                    }

                    let min_priority = u8::try_from(sub.min_priority)
                        .ok()
                        .and_then(NotificationPriority::from_int)
                        .unwrap_or(NotificationPriority::Critical);
                    if priority < min_priority {
                        return;
                    }

                    if let Err(e) =
                        validate_push_endpoint(&sub.endpoint, allow_private, check_dns).await
                    {
                        tracing::warn!(
                            endpoint = %redact_url_for_log(&sub.endpoint),
                            error = %e,
                            "Web push delivery blocked by target policy"
                        );
                        if let Some(metrics) = self.metrics.read().as_ref().cloned() {
                            metrics.record_web_push_failed();
                        }
                        return;
                    }

                    if let Err(e) = self
                        .send_to_subscription(client, &sub, event, event_log_id.as_deref())
                        .await
                    {
                        tracing::warn!(
                            endpoint = %redact_url_for_log(&sub.endpoint),
                            error = %e,
                            "Web push delivery failed"
                        );
                    }
                }
            })
            .await;
    }

    async fn send_to_subscription(
        &self,
        client: &reqwest::Client,
        sub: &WebPushSubscriptionDbModel,
        event: &NotificationEvent,
        event_log_id: Option<&str>,
    ) -> Result<()> {
        let started = Instant::now();
        let aud = push_service_audience(&sub.endpoint)?;
        let jwt = self.get_or_build_vapid_jwt(&aud)?;

        let payload_bytes =
            WebPushPayload::from_event(event, event_log_id).into_bytes_capped(MAX_PAYLOAD_BYTES)?;

        let client_pub_raw = decode_b64url(&sub.p256dh)
            .map_err(|e| Error::Other(format!("Invalid p256dh key: {}", e)))?;
        let client_auth = decode_b64url(&sub.auth)
            .map_err(|e| Error::Other(format!("Invalid auth key: {}", e)))?;
        let client_pub_raw: [u8; PUBLIC_KEY_LEN] = client_pub_raw
            .try_into()
            .map_err(|_| Error::Other("Invalid p256dh key length".to_string()))?;
        let client_auth: [u8; AUTH_SECRET_LEN] = client_auth
            .try_into()
            .map_err(|_| Error::Other("Invalid auth secret length".to_string()))?;

        let body = encrypt_aes128gcm(&payload_bytes, &client_pub_raw, &client_auth)?;

        let authorization = format!("vapid t={}, k={}", jwt, self.config.vapid_public_key_b64);

        let urgency = match event.priority() {
            NotificationPriority::Critical => "high",
            NotificationPriority::High => "normal",
            NotificationPriority::Normal => "low",
            NotificationPriority::Low => "very-low",
        };

        let request = client
            .post(&sub.endpoint)
            .header("TTL", "3600")
            // For aes128gcm, salt/server key are encoded in the payload body (RFC8291).
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", authorization)
            .header("Urgency", urgency)
            .timeout(PUSH_TIMEOUT)
            .body(body);
        let send = |request: &reqwest::RequestBuilder| {
            let request = request
                .try_clone()
                .ok_or_else(|| Error::Other("Web push request cannot be retried".to_owned()));
            async move {
                request?.send().await.map_err(|e| {
                    Error::Other(format!("Web push request failed: {}", e.without_url()))
                })
            }
        };
        let mut response = send(&request).await?;

        let status = response.status();
        if status.as_u16() == 429
            && let Some(delay) = retry_after_delay(&response)
            && delay <= Duration::from_secs(30)
        {
            tokio::time::sleep(delay).await;
            response = send(&request).await?;
        }

        let status = response.status();
        if status.is_success() {
            if (sub.next_attempt_at.is_some() || sub.last_429_at.is_some())
                && let Err(e) = self.clear_backoff(&sub.endpoint).await
            {
                tracing::warn!(
                    endpoint = %redact_url_for_log(&sub.endpoint),
                    error = %e,
                    "Failed to clear web push backoff after successful delivery"
                );
            }
            if let Some(metrics) = self.metrics.read().as_ref().cloned() {
                metrics.record_web_push_sent(started.elapsed().as_millis() as u64);
            }
            return Ok(());
        }

        if status.as_u16() == 429 {
            let delay = retry_after_delay(&response).unwrap_or(Duration::from_secs(60));
            if let Err(e) = self
                .mark_throttled(&sub.endpoint, delay.min(Duration::from_secs(3600)))
                .await
            {
                tracing::warn!(
                    endpoint = %redact_url_for_log(&sub.endpoint),
                    error = %e,
                    "Failed to persist web push throttling state"
                );
            }
            if let Some(metrics) = self.metrics.read().as_ref().cloned() {
                metrics.record_web_push_throttled();
            }
        } else if let Some(metrics) = self.metrics.read().as_ref().cloned() {
            metrics.record_web_push_failed();
        }

        // Clean up stale subscriptions.
        if status.as_u16() == 404 || status.as_u16() == 410 {
            self.delete_subscription_by_endpoint(&sub.endpoint).await?;
            tracing::info!(
                endpoint = %redact_url_for_log(&sub.endpoint),
                status = %status,
                "Deleted stale web push subscription"
            );
            if let Some(metrics) = self.metrics.read().as_ref().cloned() {
                metrics.record_web_push_stale_deleted();
            }
            return Ok(());
        }

        // The body is not reported: a push service or proxy may echo the
        // request's headers, VAPID token and proxy login included.
        Err(Error::Other(format!(
            "Web push failed: HTTP {}",
            status.as_u16()
        )))
    }

    fn get_or_build_vapid_jwt(&self, aud: &str) -> Result<String> {
        let now = jiff::Timestamp::now().as_second();
        if let Some(entry) = self.vapid_jwt_cache.get(aud)
            && entry.exp_unix - VAPID_JWT_SKEW_SECS > now
        {
            return Ok(entry.jwt.clone());
        }

        let (jwt, exp_unix) = build_vapid_jwt_with_exp(
            aud,
            &self.config.vapid_subject,
            &self.config.vapid_private_key_raw,
            VAPID_JWT_EXP_SECS,
        )?;
        self.vapid_jwt_cache.insert(
            aud.to_string(),
            CachedVapidJwt {
                jwt: jwt.clone(),
                exp_unix,
            },
        );
        Ok(jwt)
    }

    async fn mark_throttled(&self, endpoint: &str, delay: Duration) -> Result<()> {
        let now = jiff::Timestamp::now();
        let next = now
            + jiff::SignedDuration::try_from(delay)
                .unwrap_or_else(|_| jiff::SignedDuration::from_secs(60));
        let now_ms = now.as_millisecond();
        let next_ms = next.as_millisecond();

        sqlx::query(
            "UPDATE web_push_subscription SET next_attempt_at = ?, last_429_at = ?, updated_at = ? WHERE endpoint = ?",
        )
        .bind(next_ms)
        .bind(now_ms)
        .bind(now_ms)
        .bind(endpoint)
        .execute(&self.write_pool)
        .await?;

        *self.subscription_cache.write().await = None;
        Ok(())
    }

    async fn clear_backoff(&self, endpoint: &str) -> Result<()> {
        let now_ms = jiff::Timestamp::now().as_millisecond();
        sqlx::query(
            "UPDATE web_push_subscription SET next_attempt_at = NULL, last_429_at = NULL, updated_at = ? WHERE endpoint = ?",
        )
        .bind(now_ms)
        .bind(endpoint)
        .execute(&self.write_pool)
        .await?;

        *self.subscription_cache.write().await = None;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct WebPushPayload {
    title: String,
    body: String,
    url: String,
    event_type: String,
    priority: u8,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_log_id: Option<String>,
}

impl WebPushPayload {
    fn from_event(event: &NotificationEvent, event_log_id: Option<&str>) -> Self {
        Self {
            title: event.title(),
            body: event.description(),
            url: "/notifications/events".to_string(),
            event_type: event.event_type().to_string(),
            priority: event.priority().as_int(),
            created_at: event.timestamp().to_string(),
            event_log_id: event_log_id.map(|s| s.to_string()),
        }
    }

    fn into_bytes_capped(mut self, max_bytes: usize) -> Result<Vec<u8>> {
        self.title = crate::utils::text::truncate_chars(&self.title, 120);
        self.body = crate::utils::text::truncate_chars(&self.body, 600);

        let bytes = serde_json::to_vec(&self)
            .map_err(|e| Error::Other(format!("Failed to serialize web push payload: {}", e)))?;
        if bytes.len() <= max_bytes {
            return Ok(bytes);
        }

        let minimal = WebPushPayload {
            title: crate::utils::text::truncate_chars(&self.title, 80),
            body: "Open rust-srec to view details.".to_string(),
            url: self.url,
            event_type: self.event_type,
            priority: self.priority,
            created_at: self.created_at,
            event_log_id: self.event_log_id,
        };

        let bytes = serde_json::to_vec(&minimal)
            .map_err(|e| Error::Other(format!("Failed to serialize web push payload: {}", e)))?;
        if bytes.len() > max_bytes {
            return Err(Error::Other(
                "Web push metadata exceeds the payload size limit".to_string(),
            ));
        }
        Ok(bytes)
    }
}

fn push_service_audience(endpoint: &str) -> Result<String> {
    let url = Url::parse(endpoint)
        .map_err(|e| Error::Other(format!("Invalid push endpoint URL: {}", e)))?;
    let host = url
        .host()
        .ok_or_else(|| Error::Other("Push endpoint missing host".to_string()))?;

    let host = match host {
        url::Host::Domain(d) => d.to_string(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => format!("[{}]", ip),
    };

    let aud = match (url.scheme(), url.port()) {
        (scheme, Some(port)) => format!("{}://{}:{}", scheme, host, port),
        (scheme, None) => format!("{}://{}", scheme, host),
    };
    Ok(aud)
}

fn retry_after_delay(response: &reqwest::Response) -> Option<Duration> {
    let header = response.headers().get("Retry-After")?;
    let value = header.to_str().ok()?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    None
}

fn decode_b64url(input: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(input.as_bytes())
}

fn encode_b64url(input: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = vec![0u8; len];
    hk.expand(info, &mut okm)
        .map_err(|_| Error::Other("HKDF expand failed".to_string()))?;
    Ok(okm)
}

fn generate_iv_for_record(nonce: &[u8], counter: usize) -> [u8; 12] {
    let mut iv = [0u8; 12];
    let offset = 12 - 8;
    iv[0..offset].copy_from_slice(&nonce[0..offset]);
    let mask = u64::from_be_bytes(nonce[offset..].try_into().unwrap());
    iv[offset..].copy_from_slice(&(mask ^ (counter as u64)).to_be_bytes());
    iv
}

fn encrypt_aes128gcm(
    plaintext: &[u8],
    remote_public_key_raw: &[u8; PUBLIC_KEY_LEN],
    auth_secret: &[u8; AUTH_SECRET_LEN],
) -> Result<Vec<u8>> {
    if plaintext.is_empty() {
        return Err(Error::Other("Web push payload cannot be empty".to_string()));
    }

    // Generate salt + local keypair.
    let mut salt = [0u8; SALT_LEN];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut salt);

    let remote_pub = p256::PublicKey::from_sec1_bytes(remote_public_key_raw)
        .map_err(|_| Error::Other("Invalid remote public key".to_string()))?;

    let local_secret = EphemeralSecret::generate_from_rng(&mut rng);
    let local_pub = p256::PublicKey::from(&local_secret);
    let local_pub_raw = local_pub.to_sec1_point(false);
    let local_pub_raw = local_pub_raw.as_bytes();
    let local_pub_raw: [u8; PUBLIC_KEY_LEN] = local_pub_raw
        .try_into()
        .map_err(|_| Error::Other("Invalid local public key length".to_string()))?;

    let shared_secret = local_secret.diffie_hellman(&remote_pub);
    let shared_secret = shared_secret.raw_secret_bytes();

    let mut ikm_info = [0u8; 14 + PUBLIC_KEY_LEN * 2];
    let prefix = IKM_INFO_PREFIX.as_bytes();
    ikm_info[0..prefix.len()].copy_from_slice(prefix);
    let mut offset = prefix.len();
    ikm_info[offset..offset + PUBLIC_KEY_LEN].copy_from_slice(remote_public_key_raw);
    offset += PUBLIC_KEY_LEN;
    ikm_info[offset..offset + PUBLIC_KEY_LEN].copy_from_slice(&local_pub_raw);

    let ikm = hkdf_sha256(auth_secret, shared_secret, &ikm_info, 32)?;
    let cek = hkdf_sha256(&salt, &ikm, KEY_INFO.as_bytes(), 16)?;
    let nonce = hkdf_sha256(&salt, &ikm, NONCE_INFO.as_bytes(), 12)?;

    let cek = Aes128Gcm::new_from_slice(&cek)
        .map_err(|_| Error::Other("Invalid CEK length".to_string()))?;
    let iv = generate_iv_for_record(&nonce, 0);

    // Minimal padding: ensure at least one padding byte (delimiter).
    let mut padded = Vec::with_capacity(plaintext.len() + 1);
    padded.extend_from_slice(plaintext);
    padded.push(2); // final record delimiter

    let ciphertext = cek
        .encrypt((&iv).into(), padded.as_slice())
        .map_err(|_| Error::Other("AES-GCM encryption failed".to_string()))?;

    // Build the aes128gcm header + ciphertext (RFC8291).
    let mut body = Vec::with_capacity(SALT_LEN + 4 + 1 + PUBLIC_KEY_LEN + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&DEFAULT_RS.to_be_bytes());
    body.push(PUBLIC_KEY_LEN as u8);
    body.extend_from_slice(&local_pub_raw);
    body.extend_from_slice(&ciphertext);

    Ok(body)
}

fn build_vapid_jwt_with_exp(
    aud: &str,
    subject: &str,
    private_key_raw: &[u8; 32],
    exp_secs: i64,
) -> Result<(String, i64)> {
    #[derive(Serialize)]
    struct Claims<'a> {
        aud: &'a str,
        exp: u64,
        sub: &'a str,
    }

    let header = serde_json::json!({ "typ": "JWT", "alg": "ES256" });
    let exp_unix = (jiff::Timestamp::now() + jiff::SignedDuration::from_secs(exp_secs)).as_second();
    let claims = Claims {
        aud,
        exp: exp_unix as u64,
        sub: subject,
    };

    let header_b64 = encode_b64url(
        serde_json::to_string(&header)
            .map_err(|e| Error::Other(format!("JWT header serialization failed: {}", e)))?
            .as_bytes(),
    );
    let claims_b64 = encode_b64url(
        serde_json::to_string(&claims)
            .map_err(|e| Error::Other(format!("JWT claims serialization failed: {}", e)))?
            .as_bytes(),
    );

    let signing_input = format!("{}.{}", header_b64, claims_b64);

    let signing_key = SigningKey::from_bytes(private_key_raw.into())
        .map_err(|_| Error::Other("Invalid VAPID private key".to_string()))?;
    let sig: p256::ecdsa::Signature = signing_key.sign(signing_input.as_bytes());
    let sig_b64 = encode_b64url(&sig.to_bytes());

    Ok((format!("{}.{}", signing_input, sig_b64), exp_unix))
}

/// Whether `host` is a loopback address (`localhost`, `127.0.0.0/8`, `::1`),
/// where plain `http://` Web Push endpoints are permitted for local development
/// and testing.
fn is_loopback_push_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(
        (a, b, c),
        (0, _, _)
            | (10, _, _)
            | (100, 64..=127, _)
            | (127, _, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0, 0)
            | (192, 0, 2)
            | (192, 168, _)
            | (198, 18..=19, _)
            | (198, 51, 100)
            | (203, 0, 113)
            | (224..=255, _, _)
    )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let [s0, s1, s2, s3, _, _, s6, s7] = address.segments();
    let ipv4 = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    match (s0, s1) {
        (0x2001, 0x0db8) => false,
        (0x2001, 0x0000) => is_public_ipv4(ipv4(s2, s3)) && is_public_ipv4(ipv4(!s6, !s7)),
        (0x2002, _) => is_public_ipv4(ipv4(s1, s2)),
        _ => (s0 & 0xe000) == 0x2000,
    }
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

/// Whether a delivery on `target` may be dialled from this machine, so the
/// addresses the endpoint's hostname resolves to are checked first. Only a
/// saved proxy carries every request; see [`SystemProxy::connects_directly`]
/// for when the system route does not.
fn may_dial_directly(target: &ProxyTarget) -> bool {
    !matches!(target, ProxyTarget::Explicit(_))
}

struct WebPushAddressResolver;

impl reqwest::dns::Resolve for WebPushAddressResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().trim_end_matches('.');
            let addresses: Vec<std::net::SocketAddr> = lookup_host((host, 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if host.eq_ignore_ascii_case("localhost") {
                if addresses.is_empty() || addresses.iter().any(|addr| !addr.ip().is_loopback()) {
                    return Err(Box::new(std::io::Error::other(
                        "localhost resolved to a non-loopback address",
                    ))
                        as Box<dyn std::error::Error + Send + Sync>);
                }
                return Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs);
            }
            if addresses.is_empty() || addresses.iter().any(|addr| !is_public_ip(addr.ip())) {
                return Err(Box::new(std::io::Error::other(
                    "push endpoint resolved to a non-public address",
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn web_push_client(target: &ProxyTarget, allow_private_targets: bool) -> Result<reqwest::Client> {
    static CLIENTS: ClientCache<(bool, ProxyTarget)> = ClientCache::new(16);
    CLIENTS
        .get_or_try_build(&(allow_private_targets, target.clone()), || {
            install_rustls_provider();
            // Never follow redirects on push deliveries so an external push
            // service cannot 307/302 redirect POST payloads to internal hosts.
            let mut builder =
                reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
            if SystemProxy::current().connects_directly(target) {
                builder = builder.no_proxy();
                if !allow_private_targets {
                    builder = builder.dns_resolver(std::sync::Arc::new(WebPushAddressResolver));
                }
            } else {
                builder = apply_proxy(builder, target);
            }
            build_client(builder)
        })
        .map_err(Into::into)
}

/// Validate a Web Push subscription endpoint URL against SSRF rules.
///
/// Loopback (`http://localhost`, `http://127.0.0.1`, `http://[::1]`) remains
/// allowed for local development and tests; all non-loopback private,
/// link-local, and metadata addresses (`169.254.169.254`, RFC1918, etc.) are
/// rejected unless `allow_private_targets` is explicitly enabled.
///
/// With `check_dns`, a hostname must also resolve to public addresses only.
async fn validate_push_endpoint(
    endpoint: &str,
    allow_private_targets: bool,
    check_dns: bool,
) -> Result<Url> {
    let url = Url::parse(endpoint)
        .map_err(|e| Error::validation(format!("Invalid endpoint URL: {e}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::validation(
            "subscription.endpoint must not contain credentials",
        ));
    }
    let host = url
        .host()
        .ok_or_else(|| Error::validation("subscription.endpoint missing host"))?;
    let is_loopback = is_loopback_push_host(&host);
    if url.scheme() != "https"
        && !(url.scheme() == "http" && (is_loopback || allow_private_targets))
    {
        return Err(Error::validation(
            "subscription.endpoint must use https scheme (or http for localhost)",
        ));
    }
    if allow_private_targets || is_loopback {
        return Ok(url);
    }

    let domain = match host {
        url::Host::Domain(domain) => domain,
        url::Host::Ipv4(ip) if is_public_ipv4(ip) => return Ok(url),
        url::Host::Ipv6(ip) if is_public_ipv6(ip) => return Ok(url),
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => {
            return Err(Error::validation(
                "subscription.endpoint host is not allowed",
            ));
        }
    };

    let normalized = domain.trim_end_matches('.');
    if normalized.to_ascii_lowercase().ends_with(".localhost") {
        return Err(Error::validation(
            "subscription.endpoint host is not allowed",
        ));
    }

    if check_dns {
        let port = url
            .port_or_known_default()
            .ok_or_else(|| Error::validation("subscription.endpoint port is required"))?;
        let addresses: Vec<std::net::SocketAddr> = lookup_host((normalized, port))
            .await
            .map_err(|_| Error::validation("subscription.endpoint hostname could not be resolved"))?
            .collect();
        if addresses.is_empty() || addresses.iter().any(|addr| !is_public_ip(addr.ip())) {
            return Err(Error::validation(
                "subscription.endpoint host is not allowed",
            ));
        }
    }

    Ok(url)
}
