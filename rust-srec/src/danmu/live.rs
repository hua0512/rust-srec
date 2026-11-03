//! Live chat for the player: each viewer socket gets its own provider
//! connection, forwarded as small JSON batches. Nothing is recorded.

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use platforms_parser::danmaku::{DanmuItem, DanmuMessage, DanmuType, ProviderRegistry};
use platforms_parser::media::MediaInfo;
use serde::Serialize;
use tokio::sync::{Semaphore, SemaphorePermit, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::room::{DanmuRoom, resolve_room};
use super::runner::reconnect_delay;
use crate::proxies::{ResolvedRoute, SystemProxy};

/// Live danmu sockets open at once, across all principals. Each holds a
/// platform connection, so the cap bounds outbound connections too.
const MAX_SOCKETS: usize = 16;
/// Live chat is worthless late: a batch keeps only the newest items.
const MAX_BATCH_ITEMS: usize = 100;
const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

static PROVIDERS: LazyLock<ProviderRegistry> = LazyLock::new(ProviderRegistry::with_defaults);
static SOCKETS: Semaphore = Semaphore::const_new(MAX_SOCKETS);

/// Close codes the player distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The handle is unknown, expired, or belongs to another principal.
    UnknownHandle = 4404,
    /// The platform has no usable danmu connection for this room.
    Unavailable = 4422,
    /// Every live danmu slot is in use.
    TooManySockets = 4429,
}

impl Refusal {
    fn reason(self) -> &'static str {
        match self {
            Self::UnknownHandle => "unknown or expired danmu handle",
            Self::Unavailable => "danmu unavailable for this room",
            Self::TooManySockets => "too many live danmu sockets",
        }
    }
}

/// What a danmu handle stands for. It holds the room's cookies, so it
/// deliberately implements neither `Debug` nor `Serialize`.
pub(crate) struct LiveDanmuTarget {
    room: DanmuRoom,
    /// Resolved to a danmu proxy when a socket opens, like a recording does
    /// when its collector starts.
    route: ResolvedRoute,
}

impl LiveDanmuTarget {
    /// The danmu of the live stream on page `url`, or `None` when the stream
    /// is offline, its platform has no danmu provider, or its room id cannot
    /// be resolved.
    pub(crate) fn resolve(
        url: &str,
        media: &MediaInfo,
        cookies: Option<String>,
        route: ResolvedRoute,
    ) -> Option<Self> {
        if !media.is_live {
            return None;
        }
        let extras = media
            .extras
            .as_ref()
            .map(|extras| extras.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        match resolve_room(&PROVIDERS, url, cookies, extras, None) {
            Ok(room) => Some(Self { room, route }),
            Err(error) => {
                debug!(%error, "live danmu unavailable for parsed stream");
                None
            }
        }
    }
}

/// A connection ready to serve, holding its slot under the global cap.
pub(crate) struct Admitted {
    room: DanmuRoom,
    permit: SemaphorePermit<'static>,
}

/// Claim a slot and the proxy for `target`, before the socket upgrades.
pub(crate) fn admit(target: &LiveDanmuTarget) -> Result<Admitted, Refusal> {
    let proxy = target
        .route
        .danmu_proxy(SystemProxy::current())
        .map_err(|error| {
            warn!(%error, "live danmu refused: its proxy cannot be used");
            Refusal::Unavailable
        })?;
    let permit = SOCKETS.try_acquire().map_err(|_| Refusal::TooManySockets)?;
    Ok(Admitted {
        room: DanmuRoom {
            provider: target.room.provider.clone(),
            room_id: target.room.room_id.clone(),
            config: target.room.config.clone().with_proxy(proxy),
        },
        permit,
    })
}

/// Close a socket that was refused before it upgraded.
pub(crate) async fn refuse(mut socket: WebSocket, refusal: Refusal) {
    let frame = CloseFrame {
        code: refusal as u16,
        reason: refusal.reason().into(),
    };
    if let Err(error) = socket.send(Message::Close(Some(frame))).await {
        debug!(%error, "live danmu: closing a refused socket failed");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LinkState {
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ItemKind {
    Chat,
    Superchat,
}

#[derive(Debug, Serialize)]
struct LiveItem {
    text: String,
    color: Option<u32>,
    kind: ItemKind,
    user: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame<'a> {
    Batch { items: &'a [LiveItem] },
    Status { state: LinkState },
}

/// Queue `message` for the next batch, dropping the oldest item when full.
/// Only chat and super chats with text are shown.
fn enqueue(pending: &mut VecDeque<LiveItem>, message: DanmuMessage) {
    let kind = match message.message_type {
        DanmuType::Chat => ItemKind::Chat,
        DanmuType::SuperChat => ItemKind::Superchat,
        _ => return,
    };
    let text = message.content.trim();
    if text.is_empty() {
        return;
    }
    if pending.len() >= MAX_BATCH_ITEMS {
        pending.pop_front();
    }
    pending.push_back(LiveItem {
        text: text.to_owned(),
        color: message.rgb_color(),
        kind,
        user: message.username,
    });
}

/// Serve live danmu on `socket` until the client leaves.
///
/// The provider connection lives in its own task that a drop guard cancels,
/// so it is disconnected even when this future is dropped mid-send, as
/// happens when the socket's credential is revoked.
pub(crate) async fn serve(socket: WebSocket, admitted: Admitted) {
    let pending = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BATCH_ITEMS)));
    let (state_tx, mut state_rx) = watch::channel(LinkState::Connecting);
    let cancel = CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    tokio::spawn(forward(admitted, pending.clone(), state_tx, cancel));

    let (mut sender, mut receiver) = socket.split();
    let mut flush = tokio::time::interval(FLUSH_INTERVAL);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
        HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut awaiting_pong = false;
    loop {
        let outgoing = tokio::select! {
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Pong(_))) => {
                    awaiting_pong = false;
                    continue;
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => continue,
            },
            changed = state_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = *state_rx.borrow_and_update();
                text_frame(&Frame::Status { state })
            }
            _ = flush.tick() => {
                let items: Vec<LiveItem> = pending.lock().drain(..).collect();
                if items.is_empty() {
                    continue;
                }
                text_frame(&Frame::Batch { items: &items })
            }
            _ = heartbeat.tick() => {
                if awaiting_pong {
                    debug!("live danmu client stopped answering pings");
                    break;
                }
                awaiting_pong = true;
                Some(Message::Ping(Bytes::new()))
            }
        };
        if let Some(message) = outgoing
            && sender.send(message).await.is_err()
        {
            break;
        }
    }
}

fn text_frame(frame: &Frame<'_>) -> Option<Message> {
    match serde_json::to_string(frame) {
        Ok(text) => Some(Message::Text(text.into())),
        Err(error) => {
            warn!(%error, "live danmu: failed to encode a frame");
            None
        }
    }
}

/// Keep a provider connection open until `cancel`, reconnecting with the
/// recorder's back-off whenever the provider's transport gives up.
async fn forward(
    admitted: Admitted,
    pending: Arc<Mutex<VecDeque<LiveItem>>>,
    state: watch::Sender<LinkState>,
    cancel: CancellationToken,
) {
    // The slot is held until the provider is disconnected, not merely until
    // the socket closes.
    let Admitted {
        room,
        permit: _permit,
    } = admitted;
    let DanmuRoom {
        provider,
        room_id,
        config,
    } = room;
    let mut attempt = 0u32;
    loop {
        let connected = tokio::select! {
            _ = cancel.cancelled() => return,
            connected = provider.connect(&room_id, config.clone()) => connected,
        };
        match connected {
            Ok(mut stream) => {
                state.send_replace(LinkState::Connected);
                let cancelled = loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break true,
                        item = stream.items.recv() => match item {
                            Some(DanmuItem::Message(message)) => {
                                attempt = 0;
                                enqueue(&mut pending.lock(), message);
                            }
                            Some(DanmuItem::Control(_)) => {}
                            None => break false,
                        },
                    }
                };
                if let Err(error) = provider.disconnect(&mut stream.connection).await {
                    debug!(%room_id, %error, "live danmu: disconnect failed");
                }
                if cancelled {
                    return;
                }
            }
            Err(error) => warn!(%room_id, %error, "live danmu: connect failed"),
        }
        state.send_replace(LinkState::Reconnecting);
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(reconnect_delay(attempt)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_keep_the_newest_chat_and_super_chats_only() {
        let mut pending = VecDeque::new();
        enqueue(
            &mut pending,
            DanmuMessage::gift("g", "u", "Giver", "rocket", 1),
        );
        enqueue(&mut pending, DanmuMessage::chat("e", "u", "User", "   "));
        enqueue(
            &mut pending,
            DanmuMessage::super_chat("s", "u", "Patron", " thanks ", 30).with_color("#FF8800"),
        );
        for index in 0..MAX_BATCH_ITEMS {
            let chat = DanmuMessage::chat("c", "u", "", format!("chat {index}"));
            enqueue(&mut pending, chat.with_color("not a colour"));
        }
        assert_eq!(pending.len(), MAX_BATCH_ITEMS);
        assert_eq!(pending[0].text, "chat 0", "the oldest item was dropped");
        assert_eq!(pending[0].color, None);

        let mut pending = VecDeque::new();
        enqueue(
            &mut pending,
            DanmuMessage::super_chat("s", "u", "Patron", " thanks ", 30).with_color("#FF8800"),
        );
        let items: Vec<_> = pending.into_iter().collect();
        assert_eq!(
            serde_json::to_value(Frame::Batch { items: &items }).unwrap(),
            serde_json::json!({"type": "batch", "items": [
                {"text": "thanks", "color": 0xFF8800, "kind": "superchat", "user": "Patron"},
            ]})
        );
    }
}
