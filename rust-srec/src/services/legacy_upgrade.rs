//! Application-level startup data conversions that run after SQLite schema
//! migrations.
//!
//! Each conversion owns a marker table installed by the corresponding SQL
//! migration and drops that marker in the same transaction once applied.

pub(crate) mod credentials;
pub(crate) mod proxies;

use crate::Result;
use crate::database::DbPool;

/// Run SQLite schema migrations followed by any pending legacy data
/// conversions.
pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    // Unit tests convert as if no proxy variables were set, whatever the
    // developer's shell exports.
    let environment_proxy = !cfg!(test) && crate::proxies::SystemProxy::current().detected();
    run_migrations_with_environment_proxy(pool, environment_proxy).await
}

/// [`run_migrations`] with `environment_proxy` standing for whether proxy
/// environment variables are set, which the proxy settings conversion reads.
#[doc(hidden)]
pub async fn run_migrations_with_environment_proxy(
    pool: &DbPool,
    environment_proxy: bool,
) -> Result<()> {
    crate::database::run_migrations(pool).await?;
    // Data conversions that need application logic run once the schema they
    // target exists; each owns a marker table installed by its migration.
    credentials::run(pool).await?;
    proxies::run(pool, environment_proxy).await?;
    Ok(())
}
