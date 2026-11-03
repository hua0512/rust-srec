//! Shares a recording's danmu connection with live viewers of the same room,
//! so watching a recorded room opens no second platform connection.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use platforms_parser::danmaku::DanmuMessage;
use tokio::sync::broadcast;

/// Messages a slow viewer may fall behind by before it skips ahead.
const TAP_CAPACITY: usize = 256;

/// Provider platform and room id, as `resolve_room` returns them.
type RoomKey = (String, String);

/// The rooms recordings are collecting, each with a channel of its chat and
/// super chats.
#[derive(Default)]
pub struct DanmuTaps {
    rooms: Mutex<HashMap<RoomKey, broadcast::Sender<Arc<DanmuMessage>>>>,
}

impl DanmuTaps {
    /// Claim the room for one collection. The latest claim wins: new viewers
    /// follow it, while viewers of an older claim keep it until its
    /// collection ends and then subscribe again.
    pub(crate) fn register(self: &Arc<Self>, platform: &str, room_id: &str) -> TapRegistration {
        let key = (platform.to_owned(), room_id.to_owned());
        let (sender, _) = broadcast::channel(TAP_CAPACITY);
        self.rooms.lock().insert(key.clone(), sender.clone());
        TapRegistration {
            taps: Arc::clone(self),
            key,
            sender,
        }
    }

    /// Follow the room's recording, if one is collecting it. The receiver
    /// reports `Closed` once that collection ends.
    pub(crate) fn subscribe(
        &self,
        platform: &str,
        room_id: &str,
    ) -> Option<broadcast::Receiver<Arc<DanmuMessage>>> {
        self.rooms
            .lock()
            .get(&(platform.to_owned(), room_id.to_owned()))
            .map(broadcast::Sender::subscribe)
    }
}

/// A collection's claim on its room, released when dropped.
pub(crate) struct TapRegistration {
    taps: Arc<DanmuTaps>,
    key: RoomKey,
    sender: broadcast::Sender<Arc<DanmuMessage>>,
}

impl TapRegistration {
    /// Hand `message` to the room's viewers. Without viewers this costs only
    /// the subscriber count check.
    pub(crate) fn publish(&self, message: &DanmuMessage) {
        if self.sender.receiver_count() > 0 {
            // An error only means the last viewer left meanwhile.
            let _ = self.sender.send(Arc::new(message.clone()));
        }
    }
}

impl Drop for TapRegistration {
    fn drop(&mut self) {
        let mut rooms = self.taps.rooms.lock();
        if rooms
            .get(&self.key)
            .is_some_and(|sender| sender.same_channel(&self.sender))
        {
            rooms.remove(&self.key);
        }
    }
}
