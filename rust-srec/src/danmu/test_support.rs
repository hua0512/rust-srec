//! Test doubles shared by the `danmu` module's tests.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use platforms_parser::danmaku::error::Result as DanmakuResult;
use platforms_parser::danmaku::{
    ConnectionConfig, DanmakuError, DanmuConnection, DanmuItem, DanmuProvider, DanmuStream,
};
use tokio::sync::mpsc;

/// A `DanmuProvider` that hands out pre-built item channels.
///
/// Each `connect` takes the next queued receiver; once the queue is empty
/// `connect` fails, which is what drives `CollectionRunner`'s reconnect budget to
/// exhaustion. Closing a queued channel's sender simulates the real provider
/// giving up after its own `max_reconnect_attempts`.
pub(crate) struct FakeProvider {
    platform: String,
    streams: Mutex<VecDeque<mpsc::Receiver<DanmuItem>>>,
    connects: AtomicUsize,
    disconnects: AtomicUsize,
}

impl FakeProvider {
    pub(crate) fn new(streams: Vec<mpsc::Receiver<DanmuItem>>) -> Self {
        Self {
            platform: "fake".to_string(),
            streams: Mutex::new(streams.into()),
            connects: AtomicUsize::new(0),
            disconnects: AtomicUsize::new(0),
        }
    }

    /// A URL this provider claims, for tests that go through `ProviderRegistry`.
    pub(crate) const URL: &'static str = "https://fake.test/room-1";

    pub(crate) fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    pub(crate) fn disconnects(&self) -> usize {
        self.disconnects.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DanmuProvider for FakeProvider {
    fn platform(&self) -> &str {
        &self.platform
    }

    async fn connect(
        &self,
        room_id: &str,
        _config: ConnectionConfig,
    ) -> DanmakuResult<DanmuStream> {
        let attempt = self.connects.fetch_add(1, Ordering::SeqCst);
        // The guard is released by the end of this statement, before any await.
        let next = self
            .streams
            .lock()
            .expect("fake provider mutex poisoned")
            .pop_front();

        match next {
            Some(items) => {
                let mut connection = DanmuConnection::new(
                    format!("fake-{room_id}-{attempt}"),
                    &self.platform,
                    room_id,
                );
                connection.set_connected();
                Ok(DanmuStream { connection, items })
            }
            None => Err(DanmakuError::connection(
                "fake provider has no streams left",
            )),
        }
    }

    async fn disconnect(&self, connection: &mut DanmuConnection) -> DanmakuResult<()> {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
        connection.set_disconnected();
        Ok(())
    }

    fn supports_url(&self, url: &str) -> bool {
        url.contains("fake.test")
    }

    fn extract_room_id(&self, _url: &str) -> Option<String> {
        Some("room-1".to_string())
    }
}

/// A unique path under the system temp dir, for danmu XML written by tests.
pub(crate) fn temp_xml_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rust-srec-danmu-{label}-{}.xml",
        uuid::Uuid::new_v4()
    ))
}
