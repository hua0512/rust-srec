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
use tokio::sync::{Semaphore, SemaphorePermit};
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

/// The recorder's `DanmuService` owns its registry and is not reachable from
/// API routes. Providers keep no state between connections, so a registry of
/// its own gives the live socket the same providers.
static PROVIDERS: LazyLock<ProviderRegistry> = LazyLock::new(ProviderRegistry::with_defaults);
static SOCKETS: Semaphore = Semaphore::const_new(MAX_SOCKETS);

/// Close codes the player distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The handle is unknown, expired, or belongs to another principal, or
    /// its managed playback needs renewal.
    UnknownHandle = 4404,
    /// The platform has no usable danmu connection for this room.
    Unavailable = 4422,
    /// Every live danmu slot is in use.
    TooManySockets = 4429,
    /// The server could not check the handle; the same handle may work on a
    /// later attempt.
    ServerError = 1011,
}

impl Refusal {
    fn close_frame(self) -> Message {
        let reason = match self {
            Self::UnknownHandle => "unknown or expired danmu handle",
            Self::Unavailable => "danmu unavailable for this room",
            Self::TooManySockets => "too many live danmu sockets",
            Self::ServerError => "danmu handle could not be checked",
        };
        Message::Close(Some(CloseFrame {
            code: self as u16,
            reason: reason.into(),
        }))
    }
}

/// What a danmu handle stands for. It holds the room's cookies, so it
/// deliberately implements neither `Debug` nor `Serialize`.
pub(crate) struct LiveDanmuTarget {
    room: DanmuRoom,
    /// The playback context of a managed parse. Its account checks run when
    /// a socket opens and on every heartbeat, so the cookies of a disabled or
    /// replaced account stop being used.
    pub(crate) playback_handle: Option<String>,
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
            Ok(room) => Some(Self {
                room,
                playback_handle: None,
                route,
            }),
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
    if let Err(error) = socket.send(refusal.close_frame()).await {
        debug!(%error, "live danmu: closing a refused socket failed");
    }
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

/// Serve live danmu on `socket` until the client leaves or `recheck`, run on
/// every heartbeat, refuses the handle; the socket then closes with that
/// refusal's code.
///
/// The provider connection lives in its own task that a drop guard cancels,
/// so it is disconnected even when this future is dropped mid-send, as
/// happens when the socket's credential is revoked.
pub(crate) async fn serve<F>(socket: WebSocket, admitted: Admitted, mut recheck: impl FnMut() -> F)
where
    F: Future<Output = Result<(), Refusal>>,
{
    let pending = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BATCH_ITEMS)));
    let cancel = CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    tokio::spawn(forward(admitted, pending.clone(), cancel));

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
            _ = flush.tick() => {
                let items: Vec<LiveItem> = pending.lock().drain(..).collect();
                if items.is_empty() {
                    continue;
                }
                match serde_json::to_string(&Frame::Batch { items: &items }) {
                    Ok(text) => Message::Text(text.into()),
                    Err(error) => {
                        warn!(%error, "live danmu: failed to encode a batch");
                        continue;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if awaiting_pong {
                    debug!("live danmu client stopped answering pings");
                    break;
                }
                match recheck().await {
                    Ok(()) => {
                        awaiting_pong = true;
                        Message::Ping(Bytes::new())
                    }
                    Err(refusal) => {
                        // Closing anyway; a client too slow to take the
                        // close frame loses only the code.
                        let close = sender.send(refusal.close_frame());
                        if !matches!(tokio::time::timeout(HEARTBEAT_INTERVAL, close).await, Ok(Ok(()))) {
                            debug!(?refusal, "live danmu: close frame not delivered");
                        }
                        break;
                    }
                }
            }
        };
        // A client that stops reading would otherwise park this loop in
        // `send`, heartbeat included, for as long as its TCP window lasts.
        match tokio::time::timeout(HEARTBEAT_INTERVAL, sender.send(outgoing)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                debug!(%error, "live danmu: send failed");
                break;
            }
            Err(_) => {
                debug!("live danmu: send timed out; the client stopped reading");
                break;
            }
        }
    }
}

/// Keep a provider connection open until `cancel`, reconnecting with the
/// recorder's back-off whenever the provider's transport gives up.
async fn forward(
    admitted: Admitted,
    pending: Arc<Mutex<VecDeque<LiveItem>>>,
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
    // Failed attempts since the last item arrived.
    let mut attempt = 0u32;
    loop {
        let connected = tokio::select! {
            _ = cancel.cancelled() => return,
            connected = provider.connect(&room_id, config.clone()) => connected,
        };
        let cause = match connected {
            Ok(mut stream) => {
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
                "provider transport closed".to_owned()
            }
            Err(error) => error.to_string(),
        };
        attempt = attempt.saturating_add(1);
        let delay = reconnect_delay(attempt);
        let delay_ms = delay.as_millis() as u64;
        // One warning per outage; the retries that follow it are routine.
        if attempt == 1 {
            warn!(%room_id, %cause, delay_ms, "live danmu: reconnecting");
        } else {
            debug!(%room_id, %cause, attempt, delay_ms, "live danmu: reconnecting");
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
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
