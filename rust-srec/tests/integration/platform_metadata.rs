use std::collections::HashMap;
use std::sync::{Arc, Once};

use async_trait::async_trait;
use platforms_parser::extractor::batch_status::{BatchPresence, BatchStatusProbe};
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::register_platform;
use platforms_parser::extractor::platform_extractor::{Extractor, PlatformExtractor};
use platforms_parser::media::{MediaInfo, PlatformMetadata};
use regex::Regex;
use reqwest::Client;
use rust_srec::config::ConfigEventBroadcaster;
use rust_srec::database::{self, models::StreamerDbModel, repositories::*};
use rust_srec::streamer::StreamerManager;
use sqlx::SqlitePool;

type Manager = StreamerManager<SqlxStreamerRepository>;

const PLATFORM_ID: &str = "platform-metadata-test";
const SOURCE_URL: &str = "https://metadata.example.test/channels/one";

struct MetadataProbe;

#[async_trait]
impl BatchStatusProbe for MetadataProbe {
    fn max_keys(&self) -> usize {
        1
    }

    fn key_for_url(&self, url: &str, metadata: Option<&PlatformMetadata>) -> Option<String> {
        let metadata = metadata?;
        if metadata.platform != "metadatatest"
            || metadata.schema_version != 1
            || metadata.data.get("source_url")?.as_str()? != url
        {
            return None;
        }
        metadata
            .data
            .get("channel_key")?
            .as_str()
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
    }

    async fn probe(
        &self,
        _client: &Client,
        _keys: &[String],
    ) -> Result<HashMap<String, BatchPresence>, ExtractorError> {
        Err(ExtractorError::ValidationError(
            "Persistence tests do not call the network".into(),
        ))
    }
}

struct MetadataExtractor(Extractor);

#[async_trait]
impl PlatformExtractor for MetadataExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.0
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        Err(ExtractorError::ValidationError(
            "Persistence tests supply observations directly".into(),
        ))
    }
}

fn register() {
    static REGISTER: Once = Once::new();
    static PROBE: MetadataProbe = MetadataProbe;
    REGISTER.call_once(|| {
        register_platform(
            "MetadataTest",
            Regex::new(r"^https://metadata\.example\.test/channels/[a-z]+$").unwrap(),
            |url, client, _, _| {
                Box::new(MetadataExtractor(Extractor::new(
                    "MetadataTest",
                    url,
                    client,
                )))
            },
            Some(&PROBE),
        )
        .unwrap();
    });
}

async fn seed_platform(pool: &SqlitePool) {
    sqlx::query("INSERT INTO platform_config (id, platform_name) VALUES (?, 'MetadataTest')")
        .bind(PLATFORM_ID)
        .execute(pool)
        .await
        .unwrap();
}

fn observation(channel_key: &str) -> PlatformMetadata {
    PlatformMetadata {
        platform: "metadatatest".into(),
        schema_version: 1,
        data: serde_json::json!({"source_url":SOURCE_URL, "channel_key":channel_key}),
    }
}

async fn fixture() -> (
    SqlitePool,
    Arc<SqlxStreamerRepository>,
    Manager,
    ConfigEventBroadcaster,
    String,
) {
    let pool = database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    database::run_migrations(&pool).await.unwrap();
    register();
    seed_platform(&pool).await;
    let repository = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let events = ConfigEventBroadcaster::new();
    let manager = StreamerManager::new(repository.clone(), events.clone());
    let mut model = StreamerDbModel::new("Test", SOURCE_URL, PLATFORM_ID);
    // Row creation must not accept identity from a configuration/model caller.
    model.platform_metadata = Some("injected".into());
    repository.create_streamer(&model).await.unwrap();
    assert!(
        repository
            .get_streamer(&model.id)
            .await
            .unwrap()
            .platform_metadata
            .is_none()
    );
    (pool, repository, manager, events, model.id)
}

#[tokio::test]
async fn committed_observation_is_silent_hydrates_and_rejects_old_snapshot() {
    let (pool, repository, manager, events, id) = fixture().await;
    let source = manager.get_streamer_snapshot(&id).unwrap();
    let mut receiver = events.subscribe();
    let verified = database::time::now_ms();
    assert!(
        manager
            .observe_platform_metadata(&source, observation("456"), verified)
            .await
            .unwrap()
    );
    let saved = repository.get_streamer(&id).await.unwrap();
    let cached = manager.get_streamer_snapshot(&id).unwrap();
    assert_eq!(saved.platform_metadata, cached.platform_metadata);
    assert_eq!(saved.updated_at, source.updated_at.as_millisecond());
    assert!(receiver.try_recv().is_err());
    assert!(
        !manager
            .observe_platform_metadata(&cached, observation("456"), verified + 1)
            .await
            .unwrap()
    );
    assert!(
        !manager
            .observe_platform_metadata(&source, observation("789"), verified + 1)
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .get_streamer(&id)
            .await
            .unwrap()
            .platform_metadata,
        saved.platform_metadata
    );

    // A new manager and committed writer hydrate their lookup solely from SQLite.
    let restarted = StreamerManager::new(
        Arc::new(SqlxStreamerRepository::new(pool.clone(), pool)),
        ConfigEventBroadcaster::new(),
    );
    restarted.hydrate().await.unwrap();
    assert_eq!(
        restarted
            .get_streamer_snapshot(&id)
            .unwrap()
            .platform_metadata,
        saved.platform_metadata
    );
}

#[tokio::test]
async fn edits_preserve_current_document_and_source_changes_invalidate_it() {
    let (_pool, repository, manager, _events, id) = fixture().await;
    let mut stale_model = repository.get_streamer(&id).await.unwrap();
    let source = manager.get_streamer_snapshot(&id).unwrap();
    let verified = database::time::now_ms();
    assert!(
        manager
            .observe_platform_metadata(&source, observation("456"), verified)
            .await
            .unwrap()
    );
    let saved = repository
        .get_streamer(&id)
        .await
        .unwrap()
        .platform_metadata;
    stale_model.name = "Renamed".into();
    stale_model.platform_metadata = Some("injected".into());
    repository.update_streamer(&stale_model).await.unwrap();
    assert_eq!(
        repository
            .get_streamer(&id)
            .await
            .unwrap()
            .platform_metadata,
        saved
    );
    let old = manager.get_streamer_snapshot(&id).unwrap();
    stale_model.url = "https://metadata.example.test/channels/another".into();
    repository.update_streamer(&stale_model).await.unwrap();
    assert!(
        repository
            .get_streamer(&id)
            .await
            .unwrap()
            .platform_metadata
            .is_none()
    );
    assert!(
        !manager
            .observe_platform_metadata(&old, observation("456"), verified + 1)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn platform_change_and_deletion_reject_observations() {
    let (_pool, repository, manager, _events, id) = fixture().await;
    let source = manager.get_streamer_snapshot(&id).unwrap();
    let verified = database::time::now_ms();
    assert!(
        manager
            .observe_platform_metadata(&source, observation("456"), verified)
            .await
            .unwrap()
    );
    let source = manager.get_streamer_snapshot(&id).unwrap();
    let mut model = repository.get_streamer(&id).await.unwrap();
    model.platform_config_id = "platform-twitch".into();
    repository.update_streamer(&model).await.unwrap();
    assert!(
        repository
            .get_streamer(&id)
            .await
            .unwrap()
            .platform_metadata
            .is_none()
    );
    assert!(
        !manager
            .observe_platform_metadata(&source, observation("789"), verified + 1)
            .await
            .unwrap()
    );
    let source = manager.get_streamer_snapshot(&id).unwrap();
    repository.mark_streamer_deleted(&id).await.unwrap();
    assert!(
        !manager
            .observe_platform_metadata(&source, observation("789"), verified + 2)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn metadata_for_another_source_is_rejected_before_publication() {
    let (_pool, repository, manager, events, id) = fixture().await;
    let source = manager.get_streamer_snapshot(&id).unwrap();
    let mut receiver = events.subscribe();
    let mut wrong_source = observation("stable-key");
    wrong_source.data["source_url"] = "https://metadata.example.test/channels/another".into();
    let mut wrong_platform = observation("stable-key");
    wrong_platform.platform = "other-platform".into();
    for metadata in [wrong_source, wrong_platform] {
        assert!(
            !manager
                .observe_platform_metadata(&source, metadata, database::time::now_ms())
                .await
                .unwrap()
        );
    }
    assert!(
        repository
            .get_streamer(&id)
            .await
            .unwrap()
            .platform_metadata
            .is_none()
    );
    assert!(
        manager
            .get_streamer_snapshot(&id)
            .unwrap()
            .platform_metadata
            .is_none()
    );
    assert!(receiver.try_recv().is_err());
}
