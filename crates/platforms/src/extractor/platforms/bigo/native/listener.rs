//! A signed-in Linkd connection that reports when followed streamers go
//! live.
//!
//! The account follows each streamer and turns on its live notices; BIGO
//! then pushes a notice when one of them starts streaming. A guest cannot
//! follow, so this needs an [`account`](super::ticket::account) ticket, and
//! BIGO's one native session per account means the account must not be
//! signed in to the app at the same time.
//!
//! The caller owns everything around one connection: reconnecting with a
//! fresh ticket, and sending its subscriptions again on every new connection.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::BufMut;
use serde_json::Value;
use tokio::time::{Instant, sleep_until, timeout};
use tracing::debug;
use url::Url;

use super::linkd::{Frame, Linkd, STATUS_OK, get_blob, take};
use super::ticket::Ticket;
use super::{NativeError, Route};

const HEARTBEAT: u32 = 791;
const FOLLOW: u32 = 845_341;
const FOLLOW_ANSWER: u32 = 845_597;
const LIVE_NOTICE: u32 = 1_007_901;
const LIVE_NOTICE_ANSWER: u32 = 1_008_157;
const LIVE_START_NOTIFY: u32 = 524_823;
const LIVE_START_ACK: u32 = 525_079;
const NEW_LIVE_NOTIFY: u32 = 525_335;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
/// The connection counts as dead after this long without any frame.
const SILENCE_LIMIT: Duration = Duration::from_secs(60);
/// Bound on BIGO accepting what is queued for it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The app id requests carry.
const APP_ID: i32 = 60;
/// Follow results meaning the account follows the streamer: followed now,
/// or already.
const FOLLOWED: [i32; 2] = [0, 8];

/// A signed-in Linkd connection that receives live notices.
pub struct Listener {
    linkd: Linkd,
    seq: u32,
    /// The streamer of each follow or live-notice request still awaiting
    /// its answer, by seq.
    pending: HashMap<u32, u32>,
    heartbeat_at: Instant,
    last_frame: Instant,
}

/// What a [`Listener`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A followed streamer went live. `uid` is `None` when the push does not
    /// say who.
    Live { uid: Option<u32> },
    /// BIGO answered [`Listener::subscribe`] for `uid`.
    Subscribed {
        uid: u32,
        result: Result<(), Refusal>,
    },
}

/// A refused subscription, with BIGO's result: `None` when the answer
/// reported a failure or carried no result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A guest account gets result 5.
    Follow(Option<i32>),
    LiveNotice(Option<i32>),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (refused, result) = match self {
            Self::Follow(result) => ("follow the streamer", result),
            Self::LiveNotice(result) => ("turn on the streamer's live notices", result),
        };
        match result {
            Some(result) => write!(f, "BIGO refused to {refused} (result {result})"),
            None => write!(f, "BIGO refused to {refused} (no result)"),
        }
    }
}

impl std::error::Error for Refusal {}

impl Listener {
    /// Signs in to Linkd with an account ticket.
    pub async fn connect(route: &Route, ticket: &Ticket) -> Result<Self, NativeError> {
        let linkd = Linkd::connect(route, ticket).await?;
        let now = Instant::now();
        Ok(Self {
            linkd,
            seq: 0,
            pending: HashMap::new(),
            heartbeat_at: now + HEARTBEAT_INTERVAL,
            last_frame: now,
        })
    }

    /// Follows the streamer `uid` and turns on its live notices. The
    /// requests go out while [`next_event`](Self::next_event) runs, and the
    /// outcome arrives as [`Event::Subscribed`].
    pub fn subscribe(&mut self, uid: u32) {
        let seq = self.next_seq();
        let mut body = Vec::with_capacity(36);
        body.put_i32_le(APP_ID);
        body.put_u32_le(seq);
        body.put_u32_le(self.linkd.uid);
        // A list of one streamer.
        body.put_u32_le(1);
        body.put_u32_le(uid);
        body.put_bytes(0, 16);
        self.linkd.queue(FOLLOW, &body);
        self.pending.insert(seq, uid);
    }

    /// The next event. Meanwhile it sends the heartbeat, subscription
    /// requests and push acknowledgements, so the connection only stays
    /// alive while this is awaited. An error means the connection is gone,
    /// with every subscription still awaiting its answer.
    ///
    /// Cancel-safe.
    pub async fn next_event(&mut self) -> Result<Event, NativeError> {
        loop {
            let now = Instant::now();
            if now >= self.heartbeat_at {
                let seq = self.next_seq();
                let mut body = Vec::with_capacity(6);
                body.put_i16_le(0);
                body.put_u32_le(seq);
                self.linkd.queue(HEARTBEAT, &body);
                self.heartbeat_at = now + HEARTBEAT_INTERVAL;
            }
            timeout(WRITE_TIMEOUT, self.linkd.flush())
                .await
                .map_err(|_| NativeError::Timeout("Linkd to accept data"))??;

            let heartbeat_at = self.heartbeat_at;
            let silent_at = self.last_frame + SILENCE_LIMIT;
            tokio::select! {
                biased;
                frame = self.linkd.read_frame() => {
                    self.last_frame = Instant::now();
                    if let Some(event) = self.dispatch(frame?) {
                        return Ok(event);
                    }
                }
                () = sleep_until(heartbeat_at) => {}
                () = sleep_until(silent_at) => {
                    return Err(NativeError::Timeout("any frame from Linkd"));
                }
            }
        }
    }

    fn next_seq(&mut self) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// Handles one incoming frame. Every frame goes through here, so pushes
    /// and answers that arrive in any order are all seen. A malformed frame
    /// never ends the connection.
    fn dispatch(&mut self, frame: Frame) -> Option<Event> {
        match frame.uri {
            LIVE_START_NOTIFY => {
                let uid = match LiveStart::decode(&frame.body) {
                    Ok(push) => {
                        self.linkd
                            .queue(LIVE_START_ACK, &push.ack(self.linkd.uid, unix_time()));
                        push.uid
                    }
                    Err(error) => {
                        debug!(%error, "live notice without its header");
                        None
                    }
                };
                debug!(?uid, "live notice");
                Some(Event::Live { uid })
            }
            // Seen carrying only the streamer's avatar.
            NEW_LIVE_NOTIFY => {
                debug!("live notice without a streamer");
                Some(Event::Live { uid: None })
            }
            // App id, seq, uid, result.
            FOLLOW_ANSWER => {
                let (uid, result) = self.answered(&frame, 4, 12)?;
                if !result.is_some_and(|result| FOLLOWED.contains(&result)) {
                    debug!(uid, ?result, status = frame.status, "follow refused");
                    return Some(Event::Subscribed {
                        uid,
                        result: Err(Refusal::Follow(result)),
                    });
                }
                // Live notices apply only to followed streamers.
                let seq = self.next_seq();
                let mut body = Vec::with_capacity(20);
                body.put_i32_le(APP_ID);
                body.put_u32_le(seq);
                body.put_u32_le(self.linkd.uid);
                body.put_u32_le(uid);
                // On.
                body.put_i32_le(1);
                self.linkd.queue(LIVE_NOTICE, &body);
                self.pending.insert(seq, uid);
                None
            }
            // Seq, result.
            LIVE_NOTICE_ANSWER => {
                let (uid, result) = self.answered(&frame, 0, 4)?;
                let result = match result {
                    Some(0) => Ok(()),
                    result => Err(Refusal::LiveNotice(result)),
                };
                debug!(uid, ?result, status = frame.status, "subscription answered");
                Some(Event::Subscribed { uid, result })
            }
            // Heartbeat answers, and frames Linkd sends unasked (seen: 607511,
            // 36505, 761071, 1047), only show that the connection is alive.
            _ => None,
        }
    }

    /// The streamer a subscription answer is for, no longer pending, and the
    /// answer's result: `None` when the frame reports a failure or is too
    /// short to hold one. `None` overall, ignoring the frame, when its seq is
    /// unreadable or answers nothing pending.
    fn answered(
        &mut self,
        frame: &Frame,
        seq_at: usize,
        result_at: usize,
    ) -> Option<(u32, Option<i32>)> {
        let field = |at: usize| {
            frame
                .body
                .get(at..at + 4)
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        };
        let Some(uid) = field(seq_at)
            .map(u32::from_le_bytes)
            .and_then(|seq| self.pending.remove(&seq))
        else {
            debug!(uri = frame.uri, "answer to nothing pending");
            return None;
        };
        let result = field(result_at)
            .map(i32::from_le_bytes)
            .filter(|_| frame.status == STATUS_OK);
        Some((uid, result))
    }
}

/// A LiveStartNotify push.
struct LiveStart<'a> {
    /// App id, seq and route, which the ack repeats.
    header: &'a [u8],
    uid: Option<u32>,
    message_id: u64,
}

impl<'a> LiveStart<'a> {
    fn decode(mut body: &'a [u8]) -> Result<Self, NativeError> {
        let header = take(&mut body, 12)?;
        let (uid, message_id) = content_fields(get_blob(&mut body)?);
        Ok(Self {
            header,
            uid,
            message_id,
        })
    }

    fn ack(&self, own_uid: u32, now: u32) -> Vec<u8> {
        let mut body = Vec::with_capacity(32);
        body.put_slice(self.header);
        body.put_u32_le(own_uid);
        body.put_u64_le(self.message_id);
        body.put_u32_le(now);
        body
    }
}

/// The streamer and the message id in a LiveStartNotify's JSON content.
/// Both are in `key_reserved`, an object or a string holding one, as
/// `owner_uid` and `seqid`. The streamer is also the `uid` parameter of the
/// room link in `key_extra`. Either may be missing.
fn content_fields(content: &[u8]) -> (Option<u32>, u64) {
    let Ok(mut content) = serde_json::from_slice::<Value>(content) else {
        return (None, 0);
    };
    let reserved = match content.get_mut("key_reserved").map(Value::take) {
        Some(Value::String(text)) => serde_json::from_str(&text).ok(),
        reserved => reserved,
    };
    let field = |name: &str| match reserved.as_ref()?.get(name)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    };
    let link_uid = || {
        let link = Url::parse(content.get("key_extra")?.as_str()?).ok()?;
        let (_, uid) = link.query_pairs().find(|(name, _)| name == "uid")?;
        uid.parse().ok()
    };
    let uid = field("owner_uid")
        .and_then(|uid| u32::try_from(uid).ok())
        .or_else(link_uid);
    (uid, field("seqid").unwrap_or(0))
}

fn unix_time() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as u32)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    fn listener() -> Listener {
        let now = Instant::now();
        Listener {
            linkd: Linkd::unconnected(100_000_001),
            seq: 0,
            pending: HashMap::new(),
            heartbeat_at: now + HEARTBEAT_INTERVAL,
            last_frame: now,
        }
    }

    fn frame(uri: u32, status: u16, fields: &[i64]) -> Frame {
        let mut body = Vec::new();
        for &field in fields {
            body.put_u32_le(field as u32);
        }
        Frame {
            uri,
            status,
            body: Bytes::from(body),
        }
    }

    #[test]
    fn a_followed_streamer_then_gets_live_notices() {
        let mut listener = listener();
        for uid in [3_000_000_123, 200_000_002, 200_000_003, 200_000_004] {
            listener.subscribe(uid);
        }
        let refused = |uid, result| {
            Some(Event::Subscribed {
                uid,
                result: Err(Refusal::Follow(result)),
            })
        };

        // Follow answers: app id, seq, uid, result. "Already following"
        // counts as followed.
        let follow = |seq, result| frame(FOLLOW_ANSWER, 200, &[60, seq, 100_000_001, result]);
        assert_eq!(listener.dispatch(follow(1, 8)), None);
        assert_eq!(
            listener.dispatch(follow(2, 5)),
            refused(200_000_002, Some(5))
        );
        // An answer without a result, or with a failure status, is a
        // refusal; a stray short answer is ignored.
        let short = frame(FOLLOW_ANSWER, 200, &[60, 3]);
        assert_eq!(listener.dispatch(short), refused(200_000_003, None));
        let failed = frame(FOLLOW_ANSWER, 500, &[60, 4, 100_000_001, 0]);
        assert_eq!(listener.dispatch(failed), refused(200_000_004, None));
        assert_eq!(listener.dispatch(frame(LIVE_NOTICE_ANSWER, 200, &[])), None);

        // The live-notice request went out as seq 5; its answer is seq,
        // result.
        assert_eq!(
            listener.dispatch(frame(LIVE_NOTICE_ANSWER, 200, &[5, 0])),
            Some(Event::Subscribed {
                uid: 3_000_000_123,
                result: Ok(()),
            })
        );
    }

    #[test]
    fn a_live_start_push_names_its_streamer_and_is_acknowledged() {
        // The shape of a captured push, with every id, name and URL replaced.
        // The header values are made up.
        let content = r#"{"key_extra":"bigolive://livevideoshow?roomid=7000000000000000001&uid=3000000123","key_msg":"ID:100200300, Hello","key_push_id":2,"key_push_type":1,"key_reserved":{"is_model":1,"k_model":{"@hostname":"Host","@location":"","@title":""},"loc-avatar":"https://esx.bigo.sg/avatar.jpg","owner_uid":3000000123,"seqid":"12345678901234567890","txt_type":1000000000000000001,"poster_uid":3000000123,"poster_nickname":"Host","key_show_type":"8","channelId":"live","pri":"normal","tag":"-1"},"key_sound":"","key_title":"Host 正在直播中"}"#;
        let mut body = Vec::new();
        body.put_i32_le(60);
        body.put_i32_le(7);
        body.put_i32_le(3);
        body.put_u16_le(content.len() as u16);
        body.put_slice(content.as_bytes());

        let push = LiveStart::decode(&body).unwrap();
        assert_eq!(push.uid, Some(3_000_000_123));
        // Either source names the streamer on its own.
        for source in [
            r#""owner_uid":3000000123,"#,
            r#""key_extra":"bigolive://livevideoshow?roomid=7000000000000000001&uid=3000000123","#,
        ] {
            let (uid, _) = content_fields(content.replace(source, "").as_bytes());
            assert_eq!(uid, Some(3_000_000_123), "without {source}");
        }

        let mut ack = Vec::new();
        ack.put_slice(&body[..12]);
        ack.put_u32_le(100_000_001);
        ack.put_u64_le(12_345_678_901_234_567_890);
        ack.put_u32_le(1_795_000_000);
        assert_eq!(push.ack(100_000_001, 1_795_000_000), ack);
    }
}
