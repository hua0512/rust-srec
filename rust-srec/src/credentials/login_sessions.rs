//! Durable, principal-bound login targets and atomic local completion receipts.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::credentials::platforms::bilibili::{BilibiliCredentialManager, QrPollStatus};
use crate::database::repositories::credential_profiles::{self, CredentialProfileRepository};
use crate::{Error, Result};

use super::{CredentialMaterial, CredentialOwner, ProfileError};

const PENDING_TTL_MS: i64 = 300_000;
const RECEIPT_TTL_MS: i64 = 600_000;
/// Each QR generate or poll request is bounded separately; time spent scanning
/// the code is not part of any request.
pub(crate) const QR_REQUEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

fn provider_failure(error: super::CredentialError) -> Error {
    match error {
        super::CredentialError::RateLimited { retry_after } => {
            platforms_parser::extractor::error::ExtractorError::RateLimited {
                scope: platforms_parser::extractor::error::ThrottleScope::Unknown,
                code: None,
                retry_after,
            }
            .into()
        }
        _ => ProfileError::ProviderUnavailable("provider_failed").into(),
    }
}
/// How often expired logins drop their provider auth codes.
pub(crate) const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// An expired pending login keeps an `expired` receipt for polling but loses
/// its provider auth code; rows past the receipt window are deleted.
async fn prune_in(connection: &mut sqlx::SqliteConnection, now: i64) -> Result<u64> {
    let expired = sqlx::query("UPDATE credential_login_sessions SET state = 'expired', provider_auth_code = '' WHERE state = 'pending' AND expires_at <= ?")
        .bind(now)
        .execute(&mut *connection)
        .await?
        .rows_affected();
    let deleted = sqlx::query("DELETE FROM credential_login_sessions WHERE (state = 'completed' AND completed_at < ?) OR (state != 'completed' AND expires_at < ?)")
        .bind(now - RECEIPT_TTL_MS)
        .bind(now - RECEIPT_TTL_MS)
        .execute(connection)
        .await?
        .rows_affected();
    Ok(expired + deleted)
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialLoginTarget {
    Create {
        owner: CredentialOwner,
        platform_id: String,
        label: String,
    },
    Replace {
        profile_id: String,
        expected_version: i64,
    },
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialLoginGenerated {
    pub login_id: String,
    pub url: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CredentialLoginReceipt {
    pub status: String,
    pub profile_id: Option<String>,
    pub version: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    target: String,
    provider_auth_code: String,
    expires_at: i64,
    state: String,
    result_profile_id: Option<String>,
    result_version: Option<i64>,
}

#[derive(Clone)]
pub struct CredentialLoginSessions {
    profiles: Arc<CredentialProfileRepository>,
    pool: SqlitePool,
    write_pool: SqlitePool,
    admission: Arc<super::PlatformAdmission>,
    // Fixed stripes bound memory while serializing every poll of the same receipt.
    polls: Arc<Vec<Mutex<()>>>,
    supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
}

impl CredentialLoginSessions {
    pub fn new(
        profiles: Arc<CredentialProfileRepository>,
        pool: SqlitePool,
        write_pool: SqlitePool,
        admission: Arc<super::PlatformAdmission>,
    ) -> Self {
        Self {
            profiles,
            pool,
            write_pool,
            admission,
            polls: Arc::new((0..64).map(|_| Mutex::new(())).collect()),
            supervisor: Arc::new(
                crate::utils::task_supervisor::TaskSupervisor::for_committed_work(),
            ),
        }
    }

    pub(crate) fn with_supervisor(
        mut self,
        supervisor: Arc<crate::utils::task_supervisor::TaskSupervisor>,
    ) -> Self {
        self.supervisor = supervisor;
        self
    }

    async fn validate_target(&self, target: &CredentialLoginTarget) -> Result<String> {
        let (owner, platform_id) = match target {
            CredentialLoginTarget::Create {
                owner,
                platform_id,
                label,
            } => {
                if !(1..=128).contains(&label.trim().chars().count()) {
                    return Err(Error::validation(
                        "profile label must contain 1..128 characters",
                    ));
                }
                (owner.clone(), platform_id.clone())
            }
            CredentialLoginTarget::Replace {
                profile_id,
                expected_version,
            } => {
                let profile = self.profiles.get(profile_id).await?;
                if profile.version != *expected_version {
                    return Err(ProfileError::StaleVersion.into());
                }
                (profile.owner()?, profile.platform_config_id)
            }
        };
        let mut connection = self.pool.acquire().await?;
        let platform =
            credential_profiles::require_owner(&mut connection, &owner, &platform_id).await?;
        if !platform.eq_ignore_ascii_case("bilibili") {
            return Err(Error::validation(
                "QR login is not supported for this platform",
            ));
        }
        Ok(platform_id)
    }

    pub async fn generate(
        &self,
        principal: &str,
        target: CredentialLoginTarget,
    ) -> Result<CredentialLoginGenerated> {
        let platform_id = self.validate_target(&target).await?;
        let deadline = super::OperationDeadline::new(QR_REQUEST_BUDGET);
        self.admission
            .admit(&platform_id, deadline)
            .await
            .map_err(|_| Error::from(ProfileError::ProviderUnavailable("admission_unavailable")))?;
        let manager = BilibiliCredentialManager::new_lazy()
            .map_err(|_| Error::from(ProfileError::ProviderUnavailable("provider_unavailable")))?;
        let requested_at = crate::database::time::now_ms();
        let qr = deadline
            .run(manager.generate_qr())
            .await
            .inspect_err(|error| self.admission.observe_provider(&platform_id, error))
            .map_err(provider_failure)?;
        let now = crate::database::time::now_ms();
        let expires_at = requested_at
            + qr.expires_in.map_or(PENDING_TTL_MS, |seconds| {
                seconds.min((PENDING_TTL_MS / 1000) as u64) as i64 * 1000
            });
        if expires_at <= now {
            return Err(Error::validation(
                "QR code expired during generation; generate a new code",
            ));
        }
        let login_id = uuid::Uuid::new_v4().to_string();
        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
        prune_in(&mut tx, now).await?;
        sqlx::query("INSERT INTO credential_login_sessions(id, principal, platform_config_id, target, provider_auth_code, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&login_id).bind(principal).bind(platform_id).bind(serde_json::to_string(&target)?).bind(qr.auth_code).bind(now).bind(expires_at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(CredentialLoginGenerated {
            login_id,
            url: qr.url,
            expires_at,
        })
    }

    /// Runs on a timer, so a dialog closed before expiry does not leave the
    /// provider auth code stored until the next generation.
    pub async fn prune(&self) -> Result<u64> {
        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
        let pruned = prune_in(&mut tx, crate::database::time::now_ms()).await?;
        tx.commit().await?;
        Ok(pruned)
    }

    pub async fn poll(&self, principal: &str, login_id: &str) -> Result<CredentialLoginReceipt> {
        let stripe = login_id.bytes().fold(0usize, |value, byte| {
            value.wrapping_mul(31).wrapping_add(byte as usize)
        }) % self.polls.len();
        let deadline = super::OperationDeadline::new(QR_REQUEST_BUDGET);
        let _guard = tokio::time::timeout_at(deadline.instant(), self.polls[stripe].lock())
            .await
            .map_err(|_| Error::from(ProfileError::ProviderUnavailable("deadline_exceeded")))?;
        let row = sqlx::query_as::<_, LoginRow>(
            "SELECT * FROM credential_login_sessions WHERE id = ? AND principal = ?",
        )
        .bind(login_id)
        .bind(principal)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("CredentialLoginSession", login_id))?;
        if row.expires_at <= crate::database::time::now_ms() {
            return self.terminal(login_id, "expired").await;
        }
        if row.state != "pending" {
            return Ok(CredentialLoginReceipt {
                status: row.state,
                profile_id: row.result_profile_id,
                version: row.result_version,
            });
        }
        let target: CredentialLoginTarget = serde_json::from_str(&row.target)?;
        let platform_id = match self.validate_target(&target).await {
            Ok(id) => id,
            Err(Error::CredentialProfile(_) | Error::NotFound { .. }) => {
                return self.terminal(login_id, "conflict").await;
            }
            Err(error) => return Err(error),
        };
        self.admission
            .admit(&platform_id, deadline)
            .await
            .map_err(|_| Error::from(ProfileError::ProviderUnavailable("admission_unavailable")))?;
        let manager = BilibiliCredentialManager::new_lazy()
            .map_err(|_| Error::from(ProfileError::ProviderUnavailable("provider_unavailable")))?;
        let result = deadline
            .run(manager.poll_qr(&row.provider_auth_code))
            .await
            .inspect_err(|error| self.admission.observe_provider(&platform_id, error))
            .map_err(provider_failure)?;
        match result.status {
            QrPollStatus::Expired => self.terminal(login_id, "expired").await,
            QrPollStatus::NotScanned | QrPollStatus::ScannedNotConfirmed => {
                Ok(CredentialLoginReceipt {
                    status: if result.status == QrPollStatus::NotScanned {
                        "not_scanned"
                    } else {
                        "scanned"
                    }
                    .to_string(),
                    profile_id: None,
                    version: None,
                })
            }
            QrPollStatus::Success => {
                let cookies = result.cookies.ok_or_else(|| {
                    Error::from(ProfileError::ProviderUnavailable("provider_failed"))
                })?;
                let material = CredentialMaterial {
                    cookies,
                    refresh_token: result.refresh_token,
                    access_token: result.access_token,
                    reauth_config: None,
                };
                self.complete(principal, login_id, &material).await
            }
        }
    }

    async fn terminal(&self, login_id: &str, state: &str) -> Result<CredentialLoginReceipt> {
        sqlx::query("UPDATE credential_login_sessions SET state = ?, provider_auth_code = '' WHERE id = ? AND state = 'pending'")
            .bind(state).bind(login_id).execute(&self.write_pool).await?;
        Ok(CredentialLoginReceipt {
            status: state.to_string(),
            profile_id: None,
            version: None,
        })
    }

    /// Provider delivery is not transactional; only this local mutation and receipt are atomic.
    pub(crate) async fn complete(
        &self,
        principal: &str,
        login_id: &str,
        material: &CredentialMaterial,
    ) -> Result<CredentialLoginReceipt> {
        let service = self.clone();
        let principal = principal.to_owned();
        let login_id = login_id.to_owned();
        let material = material.clone();
        crate::database::committed_writer::own_operation(self.supervisor.clone(), async move {
            service
                .complete_inner(&principal, &login_id, &material)
                .await
        })
        .await
    }

    async fn complete_inner(
        &self,
        principal: &str,
        login_id: &str,
        material: &CredentialMaterial,
    ) -> Result<CredentialLoginReceipt> {
        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
        let row = sqlx::query_as::<_, LoginRow>(
            "SELECT * FROM credential_login_sessions WHERE id = ? AND principal = ?",
        )
        .bind(login_id)
        .bind(principal)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| Error::not_found("CredentialLoginSession", login_id))?;
        if row.state != "pending" {
            return Ok(CredentialLoginReceipt {
                status: row.state,
                profile_id: row.result_profile_id,
                version: row.result_version,
            });
        }
        let now = crate::database::time::now_ms();
        if row.expires_at <= now {
            sqlx::query("UPDATE credential_login_sessions SET state = 'expired', provider_auth_code = '' WHERE id = ?").bind(login_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(CredentialLoginReceipt {
                status: "expired".into(),
                profile_id: None,
                version: None,
            });
        }
        let target: CredentialLoginTarget = serde_json::from_str(&row.target)?;
        let changed = match target {
            CredentialLoginTarget::Create {
                owner,
                platform_id,
                label,
            } => {
                credential_profiles::create_in(
                    &mut tx,
                    &owner,
                    &platform_id,
                    &label,
                    true,
                    material,
                )
                .await
            }
            CredentialLoginTarget::Replace {
                profile_id,
                expected_version,
            } => replace_in(&mut tx, &profile_id, expected_version, material).await,
        };
        let profile = match changed {
            Ok(profile) => profile,
            Err(Error::CredentialProfile(_) | Error::NotFound { .. }) => {
                sqlx::query("UPDATE credential_login_sessions SET state = 'conflict', provider_auth_code = '' WHERE id = ?").bind(login_id).execute(&mut *tx).await?;
                tx.commit().await?;
                return Ok(CredentialLoginReceipt {
                    status: "conflict".into(),
                    profile_id: None,
                    version: None,
                });
            }
            Err(error) => return Err(error),
        };
        sqlx::query("UPDATE credential_login_sessions SET state = 'completed', provider_auth_code = '', result_profile_id = ?, result_version = ?, completed_at = ?, expires_at = ? WHERE id = ?")
            .bind(&profile.id).bind(profile.version).bind(now).bind(now + RECEIPT_TTL_MS).bind(login_id).execute(&mut *tx).await?;
        crate::database::committed_writer::prepare_owned_commit()?;
        tx.commit().await?;
        self.profiles.publish_material(profile.owner()?);
        Ok(CredentialLoginReceipt {
            status: "completed".into(),
            profile_id: Some(profile.id),
            version: Some(profile.version),
        })
    }
}

async fn replace_in(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
    expected_version: i64,
    material: &CredentialMaterial,
) -> Result<super::CredentialProfile> {
    let profile = credential_profiles::load(connection, id).await?;
    if profile.version != expected_version {
        return Err(ProfileError::StaleVersion.into());
    }
    let platform = credential_profiles::require_owner(
        connection,
        &profile.owner()?,
        &profile.platform_config_id,
    )
    .await?;
    material.validate(&platform)?;
    sqlx::query("UPDATE credential_profiles SET cookies = ?, refresh_token = ?, access_token = ?, reauth_config = ?, revision = revision + 1, version = version + 1, updated_at = ? WHERE id = ? AND version = ?")
        .bind(&material.cookies).bind(&material.refresh_token).bind(&material.access_token).bind(material.reauth_config.as_ref().map(serde_json::to_string).transpose()?)
        .bind(crate::database::time::now_ms()).bind(id).bind(expected_version).execute(&mut *connection).await?;
    sqlx::query("DELETE FROM credential_profile_health WHERE profile_id = ?")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    credential_profiles::load(connection, id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (
        SqlitePool,
        Arc<CredentialProfileRepository>,
        CredentialLoginSessions,
    ) {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repository = Arc::new(CredentialProfileRepository::new(pool.clone(), pool.clone()));
        let admission = Arc::new(super::super::PlatformAdmission::from_config(
            &crate::monitor::StreamMonitorConfig::default(),
        ));
        let service =
            CredentialLoginSessions::new(repository.clone(), pool.clone(), pool.clone(), admission);
        (pool, repository, service)
    }

    fn material(cookie: &str) -> CredentialMaterial {
        CredentialMaterial {
            cookies: cookie.into(),
            refresh_token: None,
            access_token: None,
            reauth_config: None,
        }
    }

    async fn pending(pool: &SqlitePool, id: &str, target: &CredentialLoginTarget, ttl: i64) {
        let now = crate::database::time::now_ms();
        sqlx::query("INSERT INTO credential_login_sessions(id, principal, platform_config_id, target, provider_auth_code, created_at, expires_at) VALUES (?, 'principal-a', 'platform-bilibili', ?, 'provider-secret', ?, ?)")
            .bind(id).bind(serde_json::to_string(target).unwrap()).bind(now).bind(now + ttl).execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn pruning_drops_expired_auth_codes_and_keeps_receipts_until_their_window() {
        let (pool, _repository, service) = fixture().await;
        let target = CredentialLoginTarget::Create {
            owner: CredentialOwner::Platform {
                platform_id: "platform-bilibili".into(),
            },
            platform_id: "platform-bilibili".into(),
            label: "A".into(),
        };
        pending(&pool, "abandoned", &target, -1).await;
        pending(&pool, "open", &target, PENDING_TTL_MS).await;
        pending(&pool, "old", &target, -RECEIPT_TTL_MS - 1).await;
        service.prune().await.unwrap();
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, state, provider_auth_code FROM credential_login_sessions ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![
                ("abandoned".into(), "expired".into(), String::new()),
                ("open".into(), "pending".into(), "provider-secret".into()),
            ]
        );
        assert_eq!(
            service
                .poll("principal-a", "abandoned")
                .await
                .unwrap()
                .status,
            "expired"
        );
    }

    #[tokio::test]
    async fn completion_is_atomic_idempotent_and_restart_safe() {
        let (pool, repository, service) = fixture().await;
        let published = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = published.clone();
        repository.bind_publication(Arc::new(move |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        pending(
            &pool,
            "login-a",
            &CredentialLoginTarget::Create {
                owner: CredentialOwner::Platform {
                    platform_id: "platform-bilibili".into(),
                },
                platform_id: "platform-bilibili".into(),
                label: "A".into(),
            },
            PENDING_TTL_MS,
        )
        .await;
        assert!(
            service
                .complete("principal-b", "login-a", &material("sid=b"))
                .await
                .is_err()
        );
        let first = service
            .complete("principal-a", "login-a", &material("sid=a"))
            .await
            .unwrap();
        let second = service
            .complete("principal-a", "login-a", &material("sid=changed"))
            .await
            .unwrap();
        assert_eq!(first.profile_id, second.profile_id);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let id = first.profile_id.unwrap();
        assert_eq!(repository.get(&id).await.unwrap().cookies, "sid=a");
        let auth_code: String = sqlx::query_scalar(
            "SELECT provider_auth_code FROM credential_login_sessions WHERE id = 'login-a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(auth_code.is_empty());
        let restarted = CredentialLoginSessions::new(
            repository,
            pool.clone(),
            pool.clone(),
            service.admission.clone(),
        );
        let receipt = restarted.poll("principal-a", "login-a").await.unwrap();
        assert_eq!(receipt.profile_id.as_deref(), Some(id.as_str()));
        assert_eq!(receipt.status, "completed");
        assert_eq!(published.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_completion_returns_the_same_durable_receipt() {
        let (pool, repository, service) = fixture().await;
        let owner = CredentialOwner::Platform {
            platform_id: "platform-bilibili".into(),
        };
        pending(
            &pool,
            "shared",
            &CredentialLoginTarget::Create {
                owner,
                platform_id: "platform-bilibili".into(),
                label: "A".into(),
            },
            PENDING_TTL_MS,
        )
        .await;
        let a = material("sid=a");
        let b = material("sid=b");
        let (first, second) = tokio::join!(
            service.complete("principal-a", "shared", &a),
            service.complete("principal-a", "shared", &b)
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.profile_id, second.profile_id);
        assert_eq!(first.version, second.version);
        let profile = repository
            .get(first.profile_id.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(profile.version, 1);
        assert!(matches!(profile.cookies.as_str(), "sid=a" | "sid=b"));
    }

    #[tokio::test]
    async fn stale_replacement_and_retired_owner_conflict_without_secret_changes() {
        let (pool, repository, service) = fixture().await;
        let owner = CredentialOwner::Platform {
            platform_id: "platform-bilibili".into(),
        };
        let profile = repository
            .create(&owner, owner.id(), "A", true, &material("sid=original"))
            .await
            .unwrap();
        pending(
            &pool,
            "login-a",
            &CredentialLoginTarget::Replace {
                profile_id: profile.id.clone(),
                expected_version: profile.version,
            },
            PENDING_TTL_MS,
        )
        .await;
        repository
            .update(&profile.id, profile.version, Some("Renamed"), None, None)
            .await
            .unwrap();
        let receipt = service
            .complete("principal-a", "login-a", &material("sid=stale"))
            .await
            .unwrap();
        assert_eq!(receipt.status, "conflict");
        assert_eq!(
            repository.get(&profile.id).await.unwrap().cookies,
            "sid=original"
        );
        pending(
            &pool,
            "login-b",
            &CredentialLoginTarget::Create {
                owner: CredentialOwner::Template {
                    template_id: "missing".into(),
                },
                platform_id: owner.id().into(),
                label: "B".into(),
            },
            PENDING_TTL_MS,
        )
        .await;
        assert_eq!(
            service
                .complete("principal-a", "login-b", &material("sid=b"))
                .await
                .unwrap()
                .status,
            "conflict"
        );
    }

    #[tokio::test]
    async fn expired_login_never_creates_a_profile_and_failed_bundle_rolls_back_receipt() {
        let (pool, _, service) = fixture().await;
        let target = CredentialLoginTarget::Create {
            owner: CredentialOwner::Platform {
                platform_id: "platform-bilibili".into(),
            },
            platform_id: "platform-bilibili".into(),
            label: "A".into(),
        };
        pending(&pool, "expired", &target, -1).await;
        assert_eq!(
            service
                .complete("principal-a", "expired", &material("sid=a"))
                .await
                .unwrap()
                .status,
            "expired"
        );
        pending(&pool, "invalid", &target, PENDING_TTL_MS).await;
        assert_eq!(
            service
                .complete("principal-a", "invalid", &material("bad\r\nheader"))
                .await
                .unwrap()
                .status,
            "conflict"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
