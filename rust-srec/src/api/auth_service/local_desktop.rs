//! Password-less sessions for the desktop app's in-process backend, and the
//! rules that keep the seeded default password unusable while the shell signs
//! in without one.
//!
//! Nothing here is reachable over HTTP: the desktop shell calls these methods
//! through `ServiceContainer`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jiff::Timestamp;
use tracing::{info, warn};

use super::AuthService;
use super::config::{AuthError, AuthResponse, session_expiration};
use crate::database::models::{AuthSessionDbModel, RefreshTokenDbModel, UserDbModel};

/// Local desktop sessions kept alive at once. Issuing one more revokes the
/// oldest, so sessions the webview no longer holds do not pile up until they
/// expire.
const MAX_LOCAL_DESKTOP_SESSIONS: usize = 3;

/// The account that desktop auto-login signs in as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDesktopAccount {
    pub username: String,
    /// The account still has `must_change_password`, that is the password it
    /// was created with (for the seeded `admin`, the publicly documented
    /// default). `ServiceContainer::set_local_desktop_password` replaces it.
    pub password_setup_required: bool,
}

#[derive(Debug, Clone)]
pub(super) struct LocalDesktopSessionRecord {
    pub(super) user_id: String,
    created_at_ms: i64,
}

impl AuthService {
    /// Make `authenticate` read `guard` on every password login. While it
    /// holds `true`, a correct password for an account that still has
    /// `must_change_password` is refused exactly like a wrong one.
    ///
    /// The desktop shell keeps it set while it signs the webview in without a
    /// password: nobody is then ever made to change the seeded default
    /// password, so that password must not open a session over the loopback
    /// API either. Existing sessions are not affected.
    pub(crate) fn with_must_change_password_login_guard(mut self, guard: Arc<AtomicBool>) -> Self {
        self.refuse_must_change_password_logins = guard;
        self
    }

    pub(super) fn refuses_must_change_password_logins(&self) -> bool {
        self.refuse_must_change_password_logins
            .load(Ordering::Acquire)
    }

    /// The account auto-login uses (see [`Self::issue_local_desktop_session`]),
    /// or `None` when there is no active admin.
    pub async fn local_desktop_account(&self) -> Result<Option<LocalDesktopAccount>, AuthError> {
        Ok(self
            .find_active_admin()
            .await?
            .map(|user| LocalDesktopAccount {
                username: user.username,
                password_setup_required: user.must_change_password,
            }))
    }

    /// Replace the initial password of the account auto-login uses without
    /// asking for it, and return that account's username.
    ///
    /// Only an account that still has `must_change_password` qualifies: this
    /// exists so the desktop user can retire the seeded default password
    /// before turning password sign-in on. Once a password has been set it
    /// returns [`AuthError::PasswordAlreadySet`], and changes go through
    /// [`Self::change_password`], which needs the current password.
    pub async fn set_local_desktop_password(
        &self,
        new_password: &str,
    ) -> Result<String, AuthError> {
        let user = self
            .find_active_admin()
            .await?
            .ok_or(AuthError::UserNotFound)?;
        if !user.must_change_password {
            return Err(AuthError::PasswordAlreadySet);
        }

        self.validate_password_strength(new_password)?;
        if self
            .verify_password_blocking(new_password, &user.password_hash)
            .await?
        {
            return Err(AuthError::WeakPassword(
                "New password must be different from current password".to_string(),
            ));
        }

        let new_hash = self.hash_password_blocking(new_password).await?;
        self.user_repo
            .update_password(&user.id, &new_hash, true)
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?;
        // The password-update trigger has revoked every session of the user,
        // auto-login ones included, so they are no longer tracked either.
        self.forget_local_desktop_sessions_of(&user.id);
        self.invalidate_user_state(&user.id);
        info!(
            user_id = %user.id,
            username = %user.username,
            "Set the local desktop account's password; revoked its sessions"
        );

        Ok(user.username)
    }

    /// Whether `session_id` was issued by [`Self::issue_local_desktop_session`]
    /// (or restored from the desktop shell's list). Such a session is exempt
    /// from `must_change_password`; the flag itself stays set in SQLite, so a
    /// password login for the same user still has to change it.
    pub(super) fn is_local_desktop_session(&self, session_id: &str) -> bool {
        self.local_desktop_sessions.contains_key(session_id)
    }

    pub(super) fn forget_local_desktop_session(&self, session_id: &str) {
        self.local_desktop_sessions.remove(session_id);
    }

    pub(super) fn forget_local_desktop_sessions_of(&self, user_id: &str) {
        self.local_desktop_sessions
            .retain(|_, record| record.user_id != user_id);
    }

    /// Issue an access and refresh token for an active admin without a
    /// password, and return them with that admin's username.
    ///
    /// The account named `admin` is preferred; otherwise the first active
    /// admin is used.
    pub async fn issue_local_desktop_session(&self) -> Result<(String, AuthResponse), AuthError> {
        let user = self
            .find_active_admin()
            .await?
            .ok_or(AuthError::UserNotFound)?;

        let now = Timestamp::now();
        let now_ms = now.as_millisecond();
        let roles = user.get_roles();
        let session_id = uuid::Uuid::new_v4().to_string();
        let (access_token, access_expires_at) = self
            .jwt_service
            .generate_session_token(&user.id, roles.clone(), &session_id)
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        let refresh_token = Self::generate_refresh_token();
        let mut token_model = RefreshTokenDbModel::new(
            &user.id,
            Self::hash_token(&refresh_token),
            self.refresh_expiration(now)?,
            Some("Rust-Srec Desktop (Local)".to_string()),
        );
        token_model.session_id = Some(session_id.clone());
        let session = AuthSessionDbModel {
            id: session_id.clone(),
            user_id: user.id.clone(),
            created_at: now_ms,
            expires_at: session_expiration(access_expires_at, token_model.expires_at)?,
            revoked_at: None,
        };
        if !self
            .token_repo
            .create_session(&session, &token_model, &user.password_hash)
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?
        {
            return Err(AuthError::Internal(
                "the account changed while its local desktop session was being created".to_owned(),
            ));
        }

        self.local_desktop_sessions.insert(
            session_id,
            LocalDesktopSessionRecord {
                user_id: user.id.clone(),
                created_at_ms: now_ms,
            },
        );
        self.revoke_excess_local_desktop_sessions().await;
        self.invalidate_user_state(&user.id);
        info!(
            user_id = %user.id,
            username = %user.username,
            "Issued local desktop session"
        );

        Ok((
            user.username,
            AuthResponse {
                access_token,
                refresh_token,
                token_type: "Bearer".to_string(),
                expires_in: self.config.access_token_expiration_secs,
                refresh_expires_in: self.config.refresh_token_expiration_secs,
                roles,
                must_change_password: false,
            },
        ))
    }

    async fn find_active_admin(&self) -> Result<Option<UserDbModel>, AuthError> {
        const PAGE_SIZE: i64 = 100;

        let is_active_admin =
            |user: &UserDbModel| user.is_active && user.get_roles().iter().any(|r| r == "admin");

        let named_admin = self
            .user_repo
            .find_by_username("admin")
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?;
        if let Some(user) = named_admin.filter(is_active_admin) {
            return Ok(Some(user));
        }

        let mut offset = 0_i64;
        loop {
            let page = self
                .user_repo
                .list(PAGE_SIZE, offset)
                .await
                .map_err(|error| AuthError::Database(error.to_string()))?;
            let page_len = page.len();
            if let Some(admin) = page.into_iter().find(is_active_admin) {
                return Ok(Some(admin));
            }
            if (page_len as i64) < PAGE_SIZE {
                return Ok(None);
            }
            offset += PAGE_SIZE;
        }
    }

    /// Revoke the oldest tracked sessions beyond [`MAX_LOCAL_DESKTOP_SESSIONS`].
    ///
    /// A session stays tracked until its revocation is stored, so one that
    /// could not be revoked is retried by the next call and is still covered
    /// by [`Self::revoke_local_desktop_sessions`].
    async fn revoke_excess_local_desktop_sessions(&self) {
        let mut sessions: Vec<(String, String, i64)> = self
            .local_desktop_sessions
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().user_id.clone(),
                    entry.value().created_at_ms,
                )
            })
            .collect();
        let Some(excess) = sessions.len().checked_sub(MAX_LOCAL_DESKTOP_SESSIONS) else {
            return;
        };
        sessions.sort_by_key(|(_, _, created_at_ms)| *created_at_ms);
        for (session_id, user_id, _) in sessions.into_iter().take(excess) {
            match self.token_repo.revoke_session(&user_id, &session_id).await {
                Ok(_) => self.forget_local_desktop_session(&session_id),
                Err(error) => warn!(
                    %user_id,
                    %session_id,
                    %error,
                    "Failed to revoke excess local desktop session"
                ),
            }
        }
    }

    /// Track `(session_id, user_id)` pairs that the desktop shell stored
    /// before the process last exited.
    pub fn restore_local_desktop_sessions(
        &self,
        sessions: impl IntoIterator<Item = (String, String)>,
    ) {
        let now_ms = crate::database::time::now_ms();
        for (session_id, user_id) in sessions {
            if session_id.trim().is_empty() || user_id.trim().is_empty() {
                continue;
            }
            self.local_desktop_sessions.insert(
                session_id,
                LocalDesktopSessionRecord {
                    user_id,
                    created_at_ms: now_ms,
                },
            );
        }
    }

    /// The tracked `(session_id, user_id)` pairs, for the desktop shell to
    /// store across restarts.
    pub fn list_local_desktop_sessions(&self) -> Vec<(String, String)> {
        self.local_desktop_sessions
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().user_id.clone()))
            .collect()
    }

    /// Revoke every tracked local desktop session, and every pair in
    /// `persisted`, in SQLite. Returns how many were revoked.
    ///
    /// A session is dropped from tracking only after its revocation is stored.
    /// On an error the remaining sessions stay tracked, so the caller must
    /// keep its stored list and call this again.
    pub async fn revoke_local_desktop_sessions(
        &self,
        persisted: &[(String, String)],
    ) -> Result<usize, AuthError> {
        let mut sessions: HashMap<String, String> = self
            .local_desktop_sessions
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().user_id.clone()))
            .collect();
        for (session_id, user_id) in persisted {
            if !session_id.trim().is_empty() && !user_id.trim().is_empty() {
                sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| user_id.clone());
            }
        }

        let mut revoked = 0_usize;
        for (session_id, user_id) in sessions {
            self.token_repo
                .revoke_session(&user_id, &session_id)
                .await
                .map_err(|error| AuthError::Database(error.to_string()))?;
            self.forget_local_desktop_session(&session_id);
            self.invalidate_user_state(&user_id);
            revoked += 1;
        }
        Ok(revoked)
    }
}
