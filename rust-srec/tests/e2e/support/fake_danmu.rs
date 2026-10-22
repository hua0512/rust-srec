//! Danmu for the fake platform's rooms: a provider registered with the danmu
//! registry that hands each backend connection to a room an in-process
//! channel, which the test feeds with [`FakeDanmu::chat`].
//!
//! The provider claims only the page URLs of rooms with a live [`FakeDanmu`],
//! so a room without one records no danmu, as on a platform without chat.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once};

use platforms_parser::danmaku::registry::register_provider;
use platforms_parser::danmaku::{
    ConnectionConfig, DanmakuError, DanmuConnection, DanmuItem, DanmuMessage, DanmuProvider,
    DanmuStream, Result as DanmakuResult,
};
use tokio::sync::mpsc;

use super::fake_platform::FakeRoom;
use super::{WAIT, eventually};

/// The feeds of the rooms with a [`FakeDanmu`], by page URL. The provider is
/// registered once per process, so it finds a test's room here.
static FEEDS: LazyLock<Mutex<HashMap<String, Arc<Feed>>>> = LazyLock::new(Mutex::default);

fn feed(url: &str) -> Option<Arc<Feed>> {
    FEEDS
        .lock()
        .expect("danmu feed table lock")
        .get(url)
        .cloned()
}

fn register() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| register_provider(Arc::new(FakeDanmuProvider)));
}

#[derive(Default)]
struct Feed {
    state: Mutex<FeedState>,
    messages: AtomicUsize,
}

#[derive(Default)]
struct FeedState {
    /// The item channel of the newest connection.
    sender: Option<mpsc::Sender<DanmuItem>>,
    connections: usize,
}

struct FakeDanmuProvider;

#[async_trait::async_trait]
impl DanmuProvider for FakeDanmuProvider {
    fn platform(&self) -> &str {
        "fake"
    }

    async fn connect(
        &self,
        room_id: &str,
        _config: ConnectionConfig,
    ) -> DanmakuResult<DanmuStream> {
        let feed = feed(room_id)
            .ok_or_else(|| DanmakuError::connection(format!("no fake danmu feed for {room_id}")))?;
        let (sender, items) = mpsc::channel(64);
        let attempt = {
            let mut state = feed.state.lock().expect("danmu feed lock");
            state.sender = Some(sender);
            state.connections += 1;
            state.connections
        };
        let mut connection = DanmuConnection::new(format!("fake-{attempt}"), "fake", room_id);
        connection.set_connected();
        Ok(DanmuStream { connection, items })
    }

    async fn disconnect(&self, connection: &mut DanmuConnection) -> DanmakuResult<()> {
        connection.set_disconnected();
        Ok(())
    }

    fn supports_url(&self, url: &str) -> bool {
        feed(url).is_some()
    }

    /// The page URL itself, the key of the room's feed.
    fn extract_room_id(&self, url: &str) -> Option<String> {
        Some(url.to_owned())
    }
}

/// The danmu feed of one [`FakeRoom`]. The room has danmu while this lives.
pub struct FakeDanmu {
    url: String,
    feed: Arc<Feed>,
}

impl FakeDanmu {
    /// Give `room` a danmu feed.
    pub fn of(room: &FakeRoom) -> Self {
        register();
        let url = room.url().to_owned();
        let feed = FEEDS
            .lock()
            .expect("danmu feed table lock")
            .entry(url.clone())
            .or_default()
            .clone();
        Self { url, feed }
    }

    /// How many times the backend has connected to the feed.
    pub fn connections(&self) -> usize {
        self.feed.state.lock().expect("danmu feed lock").connections
    }

    /// Wait until the backend has connected `count` times.
    pub async fn wait_for_connection(&self, count: usize) {
        eventually(
            WAIT,
            &format!("danmu connection {count} to {}", self.url),
            || async { (self.connections() >= count).then_some(()) },
        )
        .await;
    }

    /// Send a chat message from `user` on the newest connection.
    #[track_caller]
    pub fn chat(&self, user: &str, content: &str) {
        let id = self.feed.messages.fetch_add(1, Ordering::SeqCst);
        let message = DanmuMessage::chat(format!("message-{id}"), user, user, content);
        let state = self.feed.state.lock().expect("danmu feed lock");
        let sender = state.sender.as_ref().expect("no danmu connection is open");
        sender
            .try_send(DanmuItem::Message(message))
            .expect("the backend is reading the danmu connection");
    }
}

impl Drop for FakeDanmu {
    fn drop(&mut self) {
        FEEDS
            .lock()
            .expect("danmu feed table lock")
            .remove(&self.url);
    }
}
