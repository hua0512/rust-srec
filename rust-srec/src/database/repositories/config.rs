//! Configuration repository.

mod cloning;
mod writes;
pub(crate) use writes::{
    delete_engine, import_engine, import_global, import_platform, import_template,
};

use async_trait::async_trait;
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, RetentionDays,
    TemplateConfigDbModel,
};
use crate::{Error, Result};

fn validate_global_retention(config: &GlobalConfigDbModel) -> Result<()> {
    for (field, days) in [
        ("output_retention_days", config.output_retention_days),
        (
            "job_history_retention_days",
            config.job_history_retention_days,
        ),
        (
            "notification_event_log_retention_days",
            config.notification_event_log_retention_days,
        ),
    ] {
        RetentionDays::try_from(days)
            .map_err(|error| Error::config(format!("{field}: {error}")))?;
    }
    Ok(())
}

/// Configuration repository trait.
#[async_trait]
pub trait ConfigRepository: Send + Sync {
    // Global Config
    async fn get_global_config(&self) -> Result<GlobalConfigDbModel>;
    async fn update_global_config(&self, config: &GlobalConfigDbModel) -> Result<()>;
    async fn create_global_config(&self, config: &GlobalConfigDbModel) -> Result<()>;

    // Platform Config
    async fn get_platform_config(&self, id: &str) -> Result<PlatformConfigDbModel>;
    async fn get_platform_config_by_name(&self, name: &str) -> Result<PlatformConfigDbModel>;
    async fn list_platform_configs(&self) -> Result<Vec<PlatformConfigDbModel>>;
    async fn create_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()>;
    async fn update_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()>;
    async fn delete_platform_config(&self, id: &str) -> Result<()>;

    // Template Config
    async fn get_template_config(&self, id: &str) -> Result<TemplateConfigDbModel>;
    async fn get_template_config_by_name(&self, name: &str) -> Result<TemplateConfigDbModel>;
    async fn list_template_configs(&self) -> Result<Vec<TemplateConfigDbModel>>;
    async fn create_template_config(&self, config: &TemplateConfigDbModel) -> Result<()>;
    async fn clone_template_config(
        &self,
        source_id: &str,
        new_name: &str,
    ) -> Result<TemplateConfigDbModel>;
    async fn update_template_config(&self, config: &TemplateConfigDbModel) -> Result<()>;
    async fn delete_template_config(&self, id: &str) -> Result<()>;

    // Engine Config
    async fn get_engine_config(&self, id: &str) -> Result<EngineConfigurationDbModel>;
    async fn list_engine_configs(&self) -> Result<Vec<EngineConfigurationDbModel>>;
    async fn create_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()>;
    async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()>;
    async fn delete_engine_config(&self, id: &str) -> Result<()>;
}

/// SQLx implementation of ConfigRepository.
pub struct SqlxConfigRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
    writer: Arc<crate::database::CommittedWriter>,
    publication: OnceLock<Arc<dyn Fn(crate::credentials::CredentialOwner) + Send + Sync>>,
}

impl SqlxConfigRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        let writer = Arc::new(crate::database::CommittedWriter::for_invalidation(
            write_pool.clone(),
            Arc::new(crate::utils::task_supervisor::TaskSupervisor::for_committed_work()),
        ));
        Self {
            pool,
            write_pool,
            writer,
            publication: OnceLock::new(),
        }
    }

    pub(crate) fn with_committed_writer(
        mut self,
        writer: Arc<crate::database::CommittedWriter>,
    ) -> Self {
        self.writer = writer;
        self
    }

    pub(crate) fn bind_publication(
        &self,
        publication: Arc<dyn Fn(crate::credentials::CredentialOwner) + Send + Sync>,
    ) {
        self.publication.get_or_init(|| publication);
    }

    async fn write_platform_owned(
        &self,
        config: &PlatformConfigDbModel,
        create: bool,
    ) -> Result<()> {
        let model = config.clone();
        let owner = crate::credentials::CredentialOwner::Platform {
            platform_id: model.id.clone(),
        };
        let publication = self.publication.get().cloned();
        self.writer
            .transaction(
                "write platform configuration",
                move |connection| {
                    Box::pin(async move {
                        // Update never creates: a missing platform is a zero-row update.
                        if !create
                            && let Some(current) = sqlx::query_as::<_, PlatformConfigDbModel>(
                                "SELECT * FROM platform_config WHERE id = ?",
                            )
                            .bind(&model.id)
                            .fetch_optional(&mut *connection)
                            .await?
                        {
                            super::credential_profiles::guard_platform_legacy(&current, &model)?;
                        }
                        writes::write_platform(
                            connection,
                            &model,
                            if create {
                                super::row_write::WriteMode::Insert
                            } else {
                                super::row_write::WriteMode::Update
                            },
                        )
                        .await?;
                        super::credential_profiles::validate_graph(connection).await
                    })
                },
                move |_| {
                    if let Some(publication) = publication {
                        publication(owner);
                    }
                },
            )
            .await
    }

    /// Removes the owner with its own profiles in one transaction. Publication
    /// runs after commit even if the caller is cancelled, so caches and the
    /// runtime never keep the deleted owner or its profiles.
    async fn delete_owned(&self, owner: crate::credentials::CredentialOwner) -> Result<()> {
        let publication = self.publication.get().cloned();
        let published = owner.clone();
        self.writer
            .transaction(
                "delete configuration owner",
                move |connection| {
                    Box::pin(async move {
                        super::credential_profiles::delete_owner_profiles(connection, &owner)
                            .await?;
                        let query = match owner {
                            crate::credentials::CredentialOwner::Platform { .. } => {
                                "DELETE FROM platform_config WHERE id = ?"
                            }
                            crate::credentials::CredentialOwner::Template { .. } => {
                                "DELETE FROM template_config WHERE id = ?"
                            }
                            crate::credentials::CredentialOwner::Streamer { .. } => {
                                return Err(Error::validation(
                                    "streamers are removed through their retirement",
                                ));
                            }
                        };
                        sqlx::query(query)
                            .bind(owner.id())
                            .execute(&mut *connection)
                            .await?;
                        Ok(())
                    })
                },
                move |_| {
                    if let Some(publication) = publication {
                        publication(published);
                    }
                },
            )
            .await
    }

    async fn write_template_owned(
        &self,
        config: &TemplateConfigDbModel,
        create: bool,
    ) -> Result<()> {
        let model = config.clone();
        let owner = crate::credentials::CredentialOwner::Template {
            template_id: model.id.clone(),
        };
        let publication = self.publication.get().cloned();
        self.writer
            .transaction(
                "write template configuration",
                move |connection| {
                    Box::pin(async move {
                        let mut model = model;
                        // Update never creates: a missing template is a zero-row update.
                        if !create
                            && let Some(current) = sqlx::query_as::<_, TemplateConfigDbModel>(
                                "SELECT * FROM template_config WHERE id = ?",
                            )
                            .bind(&model.id)
                            .fetch_optional(&mut *connection)
                            .await?
                        {
                            super::credential_profiles::preserve_template_policies(
                                &current, &mut model,
                            )?;
                            super::credential_profiles::guard_template_legacy_in(
                                connection, &current, &model,
                            )
                            .await?;
                        }
                        let (mode, updated_at) = if create {
                            (
                                super::row_write::WriteMode::Insert,
                                model.updated_at.timestamp_millis(),
                            )
                        } else {
                            (
                                super::row_write::WriteMode::Update,
                                crate::database::time::now_ms(),
                            )
                        };
                        writes::write_template(connection, &model, mode, updated_at).await?;
                        super::credential_profiles::validate_graph(connection).await
                    })
                },
                move |_| {
                    if let Some(publication) = publication {
                        publication(owner);
                    }
                },
            )
            .await
    }
}

#[async_trait]
impl ConfigRepository for SqlxConfigRepository {
    async fn clone_template_config(
        &self,
        source_id: &str,
        new_name: &str,
    ) -> Result<TemplateConfigDbModel> {
        self.clone_template_owned(source_id, new_name).await
    }

    async fn get_global_config(&self) -> Result<GlobalConfigDbModel> {
        // Migrations seed this row. SQLx can report an interrupted worker's row
        // stream as empty; treating that as first-run initialization would insert
        // another configuration and start services with unintended defaults.
        Ok(
            sqlx::query_as::<_, GlobalConfigDbModel>("SELECT * FROM global_config LIMIT 1")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    async fn update_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        validate_global_retention(config)?;
        writes::write_global(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Update,
        )
        .await?;
        Ok(())
    }

    async fn create_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        validate_global_retention(config)?;
        writes::write_global(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Insert,
        )
        .await?;
        Ok(())
    }

    async fn get_platform_config(&self, id: &str) -> Result<PlatformConfigDbModel> {
        sqlx::query_as::<_, PlatformConfigDbModel>("SELECT * FROM platform_config WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("PlatformConfig", id))
    }

    async fn get_platform_config_by_name(&self, name: &str) -> Result<PlatformConfigDbModel> {
        sqlx::query_as::<_, PlatformConfigDbModel>(
            "SELECT * FROM platform_config WHERE platform_name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("PlatformConfig", name))
    }

    async fn list_platform_configs(&self) -> Result<Vec<PlatformConfigDbModel>> {
        let configs = sqlx::query_as::<_, PlatformConfigDbModel>(
            "SELECT * FROM platform_config ORDER BY platform_name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(configs)
    }

    async fn create_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        self.write_platform_owned(config, true).await
    }

    async fn update_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        self.write_platform_owned(config, false).await
    }

    async fn delete_platform_config(&self, id: &str) -> Result<()> {
        self.delete_owned(crate::credentials::CredentialOwner::Platform {
            platform_id: id.to_owned(),
        })
        .await
    }

    async fn get_template_config(&self, id: &str) -> Result<TemplateConfigDbModel> {
        sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("TemplateConfig", id))
    }

    async fn get_template_config_by_name(&self, name: &str) -> Result<TemplateConfigDbModel> {
        sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("TemplateConfig", name))
    }

    async fn list_template_configs(&self) -> Result<Vec<TemplateConfigDbModel>> {
        let configs = sqlx::query_as::<_, TemplateConfigDbModel>(
            "SELECT * FROM template_config ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(configs)
    }

    async fn create_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        self.write_template_owned(config, true).await
    }

    async fn update_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        self.write_template_owned(config, false).await
    }

    async fn delete_template_config(&self, id: &str) -> Result<()> {
        self.delete_owned(crate::credentials::CredentialOwner::Template {
            template_id: id.to_owned(),
        })
        .await
    }

    async fn get_engine_config(&self, id: &str) -> Result<EngineConfigurationDbModel> {
        sqlx::query_as::<_, EngineConfigurationDbModel>(
            "SELECT * FROM engine_configuration WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("EngineConfiguration", id))
    }

    async fn list_engine_configs(&self) -> Result<Vec<EngineConfigurationDbModel>> {
        let configs = sqlx::query_as::<_, EngineConfigurationDbModel>(
            "SELECT * FROM engine_configuration ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(configs)
    }

    async fn create_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        writes::write_engine(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Insert,
        )
        .await?;
        Ok(())
    }

    async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        writes::write_engine(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Update,
        )
        .await?;
        Ok(())
    }

    async fn delete_engine_config(&self, id: &str) -> Result<()> {
        writes::delete_engine(&mut *self.write_pool.acquire().await?, id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn cancelled_platform_and_template_deletes_publish_after_commit() {
        tokio::time::timeout(Duration::from_secs(5), async {
            for template in [false, true] {
                let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
                    .await
                    .unwrap();
                crate::database::run_migrations(&pool).await.unwrap();
                let repository = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
                sqlx::query(
                    "INSERT INTO template_config(id,name) VALUES ('deleted-template','Deleted')",
                )
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO platform_config(id, platform_name) VALUES ('deleted-platform', 'deleted')")
                    .execute(&pool)
                    .await
                    .unwrap();
                let (published, mut receiver) = tokio::sync::mpsc::unbounded_channel();
                repository.bind_publication(Arc::new(move |owner| {
                    published.send(owner).unwrap();
                }));
                let gate = Arc::new(crate::database::committed_writer::CommitTestGate::default());
                repository.writer.set_commit_gate(
                    crate::database::committed_writer::CommitPhase::AfterCommit,
                    Some(gate.clone()),
                );
                let task_repository = repository.clone();
                let caller = tokio::spawn(async move {
                    if template {
                        task_repository
                            .delete_template_config("deleted-template")
                            .await
                    } else {
                        task_repository
                            .delete_platform_config("deleted-platform")
                            .await
                    }
                });
                gate.started.notified().await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                gate.release.notify_one();
                let owner = receiver.recv().await.unwrap();
                assert_eq!(owner.kind(), if template { "template" } else { "platform" });
                assert!(if template {
                    repository.get_template_config("deleted-template").await.is_err()
                } else {
                    repository.get_platform_config("deleted-platform").await.is_err()
                });
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cancelled_platform_and_template_requests_publish_after_commit() {
        tokio::time::timeout(Duration::from_secs(5), async {
            for template in [false, true] {
                let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
                    .await
                    .unwrap();
                crate::database::run_migrations(&pool).await.unwrap();
                let repository = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
                sqlx::query(
                    "INSERT INTO template_config(id,name) VALUES ('committed-template','Original')",
                )
                .execute(&pool)
                .await
                .unwrap();
                let (published, mut receiver) = tokio::sync::mpsc::unbounded_channel();
                repository.bind_publication(Arc::new(move |owner| {
                    published.send(owner).unwrap();
                }));
                let gate = Arc::new(crate::database::committed_writer::CommitTestGate::default());
                repository.writer.set_commit_gate(
                    crate::database::committed_writer::CommitPhase::AfterCommit,
                    Some(gate.clone()),
                );
                let task_repository = repository.clone();
                let caller = tokio::spawn(async move {
                    if template {
                        let mut config = task_repository
                            .get_template_config("committed-template")
                            .await
                            .unwrap();
                        config.name = "Edited".into();
                        task_repository.update_template_config(&config).await
                    } else {
                        let mut config = task_repository
                            .get_platform_config("platform-huya")
                            .await
                            .unwrap();
                        config.fetch_delay_ms = Some(1234);
                        task_repository.update_platform_config(&config).await
                    }
                });
                gate.started.notified().await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                gate.release.notify_one();
                let owner = receiver.recv().await.unwrap();
                assert_eq!(owner.kind(), if template { "template" } else { "platform" });
                if template {
                    assert_eq!(
                        repository
                            .get_template_config("committed-template")
                            .await
                            .unwrap()
                            .name,
                        "Edited"
                    );
                } else {
                    assert_eq!(
                        repository
                            .get_platform_config("platform-huya")
                            .await
                            .unwrap()
                            .fetch_delay_ms,
                        Some(1234)
                    );
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn missing_global_config_is_an_error_without_inserting_defaults() {
        let pool = crate::database::init_migration_pool("sqlite::memory:")
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        sqlx::query("DELETE FROM global_config")
            .execute(&pool)
            .await
            .unwrap();
        let repo = SqlxConfigRepository::new(pool.clone(), pool.clone());

        assert!(repo.get_global_config().await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM global_config")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn empty_global_config_read_never_creates_default_rows() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let dir = tempfile::tempdir().unwrap();
            let url = format!("sqlite:{}", dir.path().join("config.db").display());
            let writer = crate::database::init_migration_pool(&url).await.unwrap();
            crate::database::run_migrations(&writer).await.unwrap();
            let reader = crate::database::init_pool_with_size(&url, 1).await.unwrap();
            let repo = SqlxConfigRepository::new(reader.clone(), writer.clone());
            let original = repo.get_global_config().await.unwrap();

            // Shadow the table on this reader only to model an interrupted
            // worker's empty row stream without depending on driver panic
            // diagnostics or cleanup. The writer still sees the saved row.
            sqlx::query(
                "CREATE TEMP VIEW global_config AS SELECT * FROM main.global_config WHERE 0",
            )
            .execute(&reader)
            .await
            .unwrap();
            assert!(matches!(
                repo.get_global_config().await,
                Err(Error::DatabaseSqlx(sqlx::Error::RowNotFound))
            ));
            let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM global_config")
                .fetch_all(&writer)
                .await
                .unwrap();
            assert_eq!(ids, vec![original.id]);
            sqlx::query("DROP VIEW temp.global_config")
                .execute(&reader)
                .await
                .unwrap();
            let recovered = repo.get_global_config().await.unwrap();
            assert_eq!(recovered.id, ids[0]);
            assert_eq!(recovered.output_folder, original.output_folder);
            reader.close().await;
            writer.close().await;
        })
        .await
        .expect("empty-read regression must finish");
    }

    #[test]
    fn global_retention_validation_rejects_negative_values() {
        let config = GlobalConfigDbModel {
            job_history_retention_days: -1,
            ..GlobalConfigDbModel::default()
        };

        let error = validate_global_retention(&config).expect_err("negative retention");
        assert!(error.to_string().contains("job_history_retention_days"));
    }
}
