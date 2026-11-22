//! Password hashing, verification, strength validation, and password changes.

#[cfg(test)]
use std::sync::atomic::Ordering;

use argon2::{
    Argon2, Params,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use tokio::sync::OwnedSemaphorePermit;
use tracing::{debug, info, warn};

use super::AuthService;
use super::config::AuthError;

/// Bounds on the number of Argon2id hashes running at once; see
/// `AuthService::password_work_permits`.
const MIN_PASSWORD_WORK_PERMITS: usize = 2;
const MAX_PASSWORD_WORK_PERMITS: usize = 8;

/// Permits for `password_work_permits`, scaled to the host but capped so a
/// burst of logins cannot occupy the whole `spawn_blocking` pool.
pub(super) fn password_work_permits() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(MIN_PASSWORD_WORK_PERMITS)
        .clamp(MIN_PASSWORD_WORK_PERMITS, MAX_PASSWORD_WORK_PERMITS)
}

impl AuthService {
    /// Hash a password using Argon2id with OWASP recommended parameters.
    pub fn hash_password(password: &str) -> Result<String, AuthError> {
        // OWASP recommended parameters: m=19456 (19 MiB), t=2, p=1
        let params = Params::new(19456, 2, 1, None)
            .map_err(|e| AuthError::Internal(format!("Invalid Argon2 params: {}", e)))?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let password_hash = argon2
            .hash_password(password.as_bytes())
            .map_err(|e| AuthError::Internal(format!("Password hashing failed: {}", e)))?
            .to_string();

        Ok(password_hash)
    }

    /// Verify a password against a stored hash.
    pub fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| AuthError::Internal(format!("Invalid password hash format: {}", e)))?;

        // Use default Argon2 for verification (it reads params from the hash)
        let argon2 = Argon2::default();
        Ok(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    /// Run `verify_password` on a blocking thread so the Argon2id work
    /// (m=19456, t=2) never occupies a tokio async worker.
    async fn acquire_password_work_permit(&self) -> Result<OwnedSemaphorePermit, AuthError> {
        self.password_work_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AuthError::Internal(format!("Password work semaphore closed: {}", e)))
    }

    pub(super) async fn verify_password_blocking(
        &self,
        password: &str,
        hash: &str,
    ) -> Result<bool, AuthError> {
        let permit = self.acquire_password_work_permit().await?;
        let password = password.to_owned();
        let hash = hash.to_owned();
        #[cfg(test)]
        let calls = self.password_verify_calls.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            #[cfg(test)]
            calls.fetch_add(1, Ordering::SeqCst);
            Self::verify_password(&password, &hash)
        })
        .await
        .map_err(|e| AuthError::Internal(format!("Password verification task failed: {}", e)))?
    }

    /// Run `hash_password` on a blocking thread; see `verify_password_blocking`.
    pub(super) async fn hash_password_blocking(&self, password: &str) -> Result<String, AuthError> {
        let permit = self.acquire_password_work_permit().await?;
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            Self::hash_password(&password)
        })
        .await
        .map_err(|e| AuthError::Internal(format!("Password hashing task failed: {}", e)))?
    }

    /// Validate password strength.
    pub fn validate_password_strength(&self, password: &str) -> Result<(), AuthError> {
        if password.chars().count() < self.config.min_password_length {
            return Err(AuthError::WeakPassword(format!(
                "Password must be at least {} characters",
                self.config.min_password_length
            )));
        }

        let has_letter = password.chars().any(char::is_alphabetic);
        let has_number = password.chars().any(char::is_numeric);

        if !has_letter {
            return Err(AuthError::WeakPassword(
                "Password must contain at least one letter".to_string(),
            ));
        }

        if !has_number {
            return Err(AuthError::WeakPassword(
                "Password must contain at least one number".to_string(),
            ));
        }

        Ok(())
    }

    /// Change a user's password.
    pub async fn change_password(
        &self,
        user_id: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        debug!(user_id = %user_id, "Password change attempt");
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or(AuthError::UserNotFound)?;

        if !self
            .verify_password_blocking(current_password, &user.password_hash)
            .await?
        {
            warn!(user_id = %user_id, "Password change failed: incorrect current password");
            return Err(AuthError::IncorrectCurrentPassword);
        }

        if current_password == new_password {
            return Err(AuthError::WeakPassword(
                "New password must be different from current password".to_string(),
            ));
        }

        self.validate_password_strength(new_password)?;

        let new_hash = self.hash_password_blocking(new_password).await?;

        self.user_repo
            .update_password(user_id, &new_hash, true)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;
        self.invalidate_user_state(user_id);

        // The password-update trigger revokes sessions and refresh tokens atomically.
        info!(
            user_id = %user_id,
            "Password changed; revoked all refresh tokens for user"
        );

        Ok(())
    }
}
