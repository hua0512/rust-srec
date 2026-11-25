//! Database-backed [`BaiduPcsAuthenticator`] with failure cooldown and
//! rejection notifications.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::baidupcs::{CREDENTIAL_TOOL, LOGIN_TIMEOUT, cli_lock, config_dir_key, run_login};
use crate::database::models::ToolCredentialDbModel;
use crate::database::repositories::ToolCredentialRepository;
use crate::domain::processor::{BaiduPcsAuthenticator, LoginMaterial};
use crate::domain::proxy::ProxyTarget;
use crate::notification::{NotificationEvent, NotificationService};

/// How long a config-dir key sits out of automatic re-login after a
/// rejected or errored attempt. Bounds the failed-Baidu-call rate when the
/// stored material is permanently dead (e.g. password change): at most one
/// replay per window instead of one per upload job.
const RELOGIN_COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// Failed re-login bookkeeping, keyed by [`config_dir_key`].
/// In-process only: a restart retries immediately, which is the desired
/// behavior after an operator intervenes.
struct ReloginCooldown {
    window: Duration,
    last_failure: Mutex<HashMap<String, Instant>>,
}

impl ReloginCooldown {
    fn new(window: Duration) -> Self {
        Self {
            window,
            last_failure: Mutex::new(HashMap::new()),
        }
    }

    fn should_attempt(&self, key: &str) -> bool {
        self.should_attempt_at(key, Instant::now())
    }

    fn should_attempt_at(&self, key: &str, now: Instant) -> bool {
        self.last_failure
            .lock()
            .get(key)
            .is_none_or(|failed_at| now.duration_since(*failed_at) >= self.window)
    }

    fn record_failure(&self, key: &str) {
        self.record_failure_at(key, Instant::now());
    }

    fn record_failure_at(&self, key: &str, now: Instant) {
        self.last_failure.lock().insert(key.to_string(), now);
    }

    /// Cleared on success and whenever the stored material changes
    /// (`save_credentials` / `delete_credentials`), so fresh credentials are
    /// tried immediately instead of sitting out a stale window.
    fn clear(&self, key: &str) {
        self.last_failure.lock().remove(key);
    }
}

/// Production [`BaiduPcsAuthenticator`] over the `tool_credentials` table
/// (rows scoped by [`CREDENTIAL_TOOL`]), with a per-config-dir failure
/// cooldown and a notification on rejected re-logins.
pub struct StoredLoginAuthenticator {
    repository: Arc<dyn ToolCredentialRepository>,
    cooldown: ReloginCooldown,
    /// Weak so the authenticator (held in a process-wide `OnceLock`) never
    /// keeps the notification service alive through shutdown.
    notification_service: Weak<NotificationService>,
}

impl StoredLoginAuthenticator {
    pub fn new(repository: Arc<dyn ToolCredentialRepository>) -> Self {
        Self {
            repository,
            cooldown: ReloginCooldown::new(RELOGIN_COOLDOWN),
            notification_service: Weak::new(),
        }
    }

    /// Attach the notification service used to report rejected re-logins.
    pub fn with_notification_service(mut self, service: Weak<NotificationService>) -> Self {
        self.notification_service = service;
        self
    }

    /// Emit `NotificationEvent::BaiduPcsReloginFailed`. Called only after a
    /// real re-login attempt (never for cooldown skips), so with
    /// [`RELOGIN_COOLDOWN`] this fires at most once per window per key.
    fn notify_relogin_failure(&self, key: &str, message: &str) {
        let Some(service) = self.notification_service.upgrade() else {
            return;
        };
        service.dispatch_notification(NotificationEvent::BaiduPcsReloginFailed {
            config_dir: if key.is_empty() {
                "default".to_string()
            } else {
                key.to_string()
            },
            message: crate::utils::text::truncate_chars(message.trim(), 300),
            timestamp: jiff::Timestamp::now(),
        });
    }
}

#[async_trait]
impl BaiduPcsAuthenticator for StoredLoginAuthenticator {
    async fn has_credentials(&self, config_dir: Option<&str>) -> bool {
        let key = config_dir_key(config_dir);
        match self.repository.get(CREDENTIAL_TOOL, &key).await {
            Ok(Some(row)) => LoginMaterial::from_payload(&row.payload).is_some(),
            Ok(None) => false,
            Err(e) => {
                warn!(error = %e, "Failed to look up stored BaiduPCS-Go credentials");
                false
            }
        }
    }

    async fn save_credentials(
        &self,
        config_dir: Option<&str>,
        material: &LoginMaterial,
    ) -> srec_core::CoreResult<()> {
        let key = config_dir_key(config_dir);
        let payload = serde_json::to_string(material).map_err(|e| {
            srec_core::CoreError::Other(format!("Failed to serialize BaiduPCS-Go credentials: {e}"))
        })?;
        let now_ms = jiff::Timestamp::now().as_millisecond();
        self.repository
            .upsert(&ToolCredentialDbModel {
                tool: CREDENTIAL_TOOL.to_string(),
                account_key: key.clone(),
                payload,
                created_at: now_ms,
                updated_at: now_ms,
            })
            .await?;
        // Fresh material must be tried immediately, not sit out a cooldown
        // earned by the previous credentials.
        self.cooldown.clear(&key);
        Ok(())
    }

    async fn delete_credentials(&self, config_dir: Option<&str>) -> srec_core::CoreResult<()> {
        let key = config_dir_key(config_dir);
        self.repository.delete(CREDENTIAL_TOOL, &key).await?;
        self.cooldown.clear(&key);
        Ok(())
    }

    async fn relogin(
        &self,
        binary_path: &str,
        config_dir: Option<&str>,
        proxy: &ProxyTarget,
    ) -> srec_core::CoreResult<bool> {
        let key = config_dir_key(config_dir);
        if !self.cooldown.should_attempt(&key) {
            debug!("Skipping BaiduPCS-Go re-login during failure cooldown");
            return Ok(false);
        }
        let Some(row) = self.repository.get(CREDENTIAL_TOOL, &key).await? else {
            return Ok(false);
        };
        let Some(material) = LoginMaterial::from_payload(&row.payload) else {
            return Ok(false);
        };

        let outcome = {
            let lease = Arc::new(cli_lock().clone().write_owned().await);
            run_login(
                binary_path,
                config_dir,
                proxy,
                &material,
                LOGIN_TIMEOUT,
                lease,
            )
            .await
        };
        match outcome {
            Ok(outcome) if outcome.success => {
                self.cooldown.clear(&key);
                Ok(true)
            }
            Ok(outcome) => {
                self.cooldown.record_failure(&key);
                warn!(
                    message = %outcome.message,
                    "Automatic BaiduPCS-Go re-login was rejected; stored credentials may have expired"
                );
                self.notify_relogin_failure(&key, &outcome.message);
                Ok(false)
            }
            Err(e) => {
                self.cooldown.record_failure(&key);
                self.notify_relogin_failure(&key, &e.to_string());
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relogin_cooldown_blocks_within_window_only() {
        let cooldown = ReloginCooldown::new(Duration::from_secs(3600));
        let start = Instant::now();

        assert!(cooldown.should_attempt_at("", start));
        cooldown.record_failure_at("", start);
        assert!(!cooldown.should_attempt_at("", start + Duration::from_secs(3599)));
        assert!(cooldown.should_attempt_at("", start + Duration::from_secs(3600)));
        assert!(cooldown.should_attempt_at("/other", start));

        cooldown.record_failure_at("", start);
        cooldown.clear("");
        assert!(cooldown.should_attempt_at("", start));
    }

    struct MockCredentialRepo {
        row: Option<ToolCredentialDbModel>,
        gets: std::sync::atomic::AtomicUsize,
    }

    impl MockCredentialRepo {
        fn new(row: Option<ToolCredentialDbModel>) -> Arc<Self> {
            Arc::new(Self {
                row,
                gets: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn gets(&self) -> usize {
            self.gets.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl ToolCredentialRepository for MockCredentialRepo {
        async fn get(
            &self,
            tool: &str,
            account_key: &str,
        ) -> srec_sqlite::DbResult<Option<ToolCredentialDbModel>> {
            self.gets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self
                .row
                .clone()
                .filter(|row| row.tool == tool && row.account_key == account_key))
        }

        async fn upsert(&self, _model: &ToolCredentialDbModel) -> srec_sqlite::DbResult<()> {
            Ok(())
        }

        async fn delete(&self, _tool: &str, _account_key: &str) -> srec_sqlite::DbResult<()> {
            Ok(())
        }
    }

    fn stored_row() -> ToolCredentialDbModel {
        ToolCredentialDbModel {
            tool: CREDENTIAL_TOOL.to_string(),
            account_key: String::new(),
            payload: r#"{"cookies":"BDUSS=abc; STOKEN=DEF"}"#.to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn relogin_skips_everything_during_cooldown() {
        let repo = MockCredentialRepo::new(Some(stored_row()));
        let authenticator = StoredLoginAuthenticator::new(repo.clone());
        authenticator.cooldown.record_failure("");

        let result = authenticator
            .relogin("BaiduPCS-Go", None, &ProxyTarget::System)
            .await
            .unwrap();

        assert!(!result);
        assert_eq!(repo.gets(), 0, "cooldown short-circuits before the store");
    }

    #[tokio::test]
    async fn relogin_without_stored_material_records_no_failure() {
        let repo = MockCredentialRepo::new(None);
        let authenticator = StoredLoginAuthenticator::new(repo.clone());

        let result = authenticator
            .relogin("BaiduPCS-Go", None, &ProxyTarget::System)
            .await
            .unwrap();

        assert!(!result);
        assert_eq!(repo.gets(), 1);
        assert!(authenticator.cooldown.should_attempt(""));
    }

    #[tokio::test]
    async fn saving_credentials_clears_the_cooldown() {
        let repo = MockCredentialRepo::new(None);
        let authenticator = StoredLoginAuthenticator::new(repo);
        authenticator.cooldown.record_failure("");
        assert!(!authenticator.cooldown.should_attempt(""));

        authenticator
            .save_credentials(
                None,
                &LoginMaterial {
                    cookies: Some("BDUSS=new; STOKEN=NEW".to_string()),
                    bduss: None,
                    stoken: None,
                },
            )
            .await
            .unwrap();

        assert!(authenticator.cooldown.should_attempt(""));
    }
}
