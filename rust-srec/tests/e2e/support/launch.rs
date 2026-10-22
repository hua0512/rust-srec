//! Serving a backend on a migrated database: the last step of every start,
//! a restart on an app's own data directory, and an app whose download
//! manager uses settings other than the defaults.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rust_srec::backend::{
    self, ApiServerConfig, DownloadManagerConfig, ServiceContainer, ServiceContainerConfig,
};

use super::seed::Seed;
use super::{
    ConnectRecorder, JWT_SECRET, SHUTDOWN_GRACE_PERIOD, SHUTDOWN_TIMEOUT, TestApp, logging,
};

/// Build, initialize and serve a backend on the migrated database at
/// `database_url`, recording into `output_dir`.
pub(super) async fn serve(
    database_url: &str,
    output_dir: &Path,
    downloads: DownloadManagerConfig,
) -> (Arc<ServiceContainer>, SocketAddr) {
    let (pool, write_pool) = backend::init_database_pools(database_url)
        .await
        .expect("open the runtime pools");

    let container = ServiceContainer::with_full_config(
        pool,
        write_pool,
        ServiceContainerConfig {
            cache_ttl: Duration::from_secs(3600),
            event_capacity: 256,
            download_config: downloads,
            pipeline_config: backend::PipelineManagerConfig::default(),
            api_config: ApiServerConfig {
                bind_address: "127.0.0.1".to_string(),
                port: 0,
                enable_cors: true,
                body_limit: 10 * 1024 * 1024,
            },
        },
    )
    .await
    .expect("build the service container");
    let container = Arc::new(container);

    let config = container.config_service();
    let mut global = config
        .get_global_config()
        .await
        .expect("read the global config");
    global.output_folder = output_dir.to_string_lossy().into_owned();
    config
        .update_global_config(&global)
        .await
        .expect("point recordings at the test output directory");

    let logging = logging();
    logging.apply_persisted_filter(config).await;
    container.set_logging_config(logging);
    container.initialize().await.expect("initialize services");
    let address = container
        .start_api_server_bound_with_jwt_secret(JWT_SECRET.to_string())
        .await
        .expect("start the API server");
    (container, address)
}

impl TestApp {
    /// Like [`Self::start_with`], with `downloads` as the download manager's
    /// settings, for its circuit breaker, which reads them once at startup.
    pub async fn start_with_downloads(seed: Seed, downloads: DownloadManagerConfig) -> Self {
        let dead_end = ConnectRecorder::start().await;
        let sql = seed.sql(&dead_end);
        Self::boot(&sql, Some(dead_end), true, downloads).await
    }

    /// Shut the backend down and start a new one on the same data directory,
    /// as restarting the process does: the database, the recordings and
    /// every setting stay, everything held in memory is rebuilt from them.
    /// The new backend runs the migrations on the existing database, listens
    /// on a new port and uses the default download manager settings; the
    /// admin session and the dead-end proxy carry over.
    pub async fn restart(self) -> Self {
        let Self {
            base_url: _,
            database_url,
            client,
            container,
            admin,
            output_dir,
            dead_end,
            _data_dir,
        } = self;
        container
            .shutdown_with_hard_cap(SHUTDOWN_GRACE_PERIOD, SHUTDOWN_TIMEOUT)
            .await
            .expect("backend shutdown failed");
        drop(container);

        let migration_pool = backend::init_migration_pool(&database_url)
            .await
            .expect("open the migration pool");
        rust_srec::database::run_migrations_with_environment_proxy(&migration_pool, false)
            .await
            .expect("run migrations");
        migration_pool.close().await;
        let (container, address) =
            serve(&database_url, &output_dir, DownloadManagerConfig::default()).await;
        Self {
            base_url: format!("http://{address}"),
            database_url,
            client,
            container,
            admin,
            output_dir,
            dead_end,
            _data_dir,
        }
    }
}
