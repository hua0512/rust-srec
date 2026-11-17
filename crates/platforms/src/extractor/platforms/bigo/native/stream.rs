//! Recording a room natively: a guest ticket, a Linkd room lookup, then the
//! room's video and audio from its media servers, as one FLV stream.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::LazyLock;
use std::time::Duration;

use ::flv::FlvData;
use bytes::Bytes;
use futures::Stream;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tracing::{debug, info, warn};
use url::Url;

use super::depacketize::{VideoDepacketizer, audio_frames};
use super::flv::Muxer;
use super::linkd::{Linkd, MediaServer};
use super::media::{MediaConnection, Track};
use super::{NativeError, Route, ticket};

/// Bound on reaching both media servers and logging in. The room lookup's
/// media ticket is only accepted for a few seconds (2 s to 5 s seen).
const MEDIA_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// A connection must deliver a keyframe this soon.
const KEYFRAME_TIMEOUT: Duration = Duration::from_secs(12);
/// After that, it fails when this long passes without video written.
const VIDEO_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the start of the stream waits for audio after the first
/// keyframe, so that a room with audio starts with both sequence headers. A
/// room without audio is recorded without it.
const AUDIO_WAIT: Duration = Duration::from_secs(2);
const MAX_RECONNECTS: u32 = 3;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// A connection that has delivered media this long restores the reconnects,
/// so that a room whose connections start and then die runs out of them.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);
/// How long a broadcaster whose native recording failed is left to the web
/// stream, unless the profile reads offline first. A failure before the
/// media became stable tends to repeat for the whole broadcast (an
/// unsupported codec, servers that send no video), and each attempt costs up
/// to a minute of recording; a passing problem gets the native stream again
/// soon, and the broadcaster's next stream gets it at once.
const FAILURE_MEMORY: Duration = Duration::from_secs(30 * 60);

/// Broadcasters whose last native recording failed before its media became
/// stable, by owner uid, with when it failed.
static FAILED_OWNERS: LazyLock<Mutex<FxHashMap<u32, std::time::Instant>>> =
    LazyLock::new(Mutex::default);

/// Whether a native recording of `owner_uid`'s stream failed in the last 30
/// minutes before its media became stable.
pub(crate) fn recently_failed(owner_uid: u32) -> bool {
    FAILED_OWNERS
        .lock()
        .get(&owner_uid)
        .is_some_and(|at| at.elapsed() < FAILURE_MEMORY)
}

pub(crate) fn remember_failure(owner_uid: u32) {
    let mut owners = FAILED_OWNERS.lock();
    owners.retain(|_, at| at.elapsed() < FAILURE_MEMORY);
    owners.insert(owner_uid, std::time::Instant::now());
}

/// Forgets a failed native recording of `owner_uid`'s stream, so that the
/// next one is recorded natively again.
pub(crate) fn forget_failure(owner_uid: u32) {
    FAILED_OWNERS.lock().remove(&owner_uid);
}

/// A room to record natively, written as the stream URL
/// `bigo://room/<room id>?uid=<owner uid>`. No HTTP client can open that URL;
/// only [`open`] records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRoom {
    pub room_id: u64,
    pub owner_uid: u32,
}

impl NativeRoom {
    pub fn url(&self) -> String {
        format!("bigo://room/{}?uid={}", self.room_id, self.owner_uid)
    }

    /// The room a native stream URL names; `None` for any other URL.
    pub fn parse(url: &str) -> Option<Self> {
        if !url.starts_with("bigo://") {
            return None;
        }
        let url = Url::parse(url).ok()?;
        if url.host_str() != Some("room") {
            return None;
        }
        let room_id = url.path().strip_prefix('/')?.parse::<u64>().ok()?;
        let owner_uid = url
            .query_pairs()
            .find(|(key, _)| key == "uid")?
            .1
            .parse::<u32>()
            .ok()?;
        (room_id != 0 && owner_uid != 0).then_some(Self { room_id, owner_uid })
    }
}

/// What a room lookup says about recording a room natively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomKind {
    /// A single host, with video and audio servers: recordable.
    Host,
    /// Several hosts share the room. Their mixed picture is only on the web.
    MultiGuest,
    /// The lookup lists no video or no audio servers.
    NoMedia,
}

/// Looks up `room_id` as a guest. A room that has ended is
/// [`NativeError::RoomEnded`].
pub async fn probe(room_id: u64, route: &Route) -> Result<RoomKind, NativeError> {
    let ticket = ticket::guest(route).await?;
    let channel = Linkd::connect(route, &ticket)
        .await?
        .join_channel(room_id)
        .await?;
    Ok(
        if channel.video_servers.is_empty() || channel.audio_servers.is_empty() {
            RoomKind::NoMedia
        } else if channel.multi_guest() {
            RoomKind::MultiGuest
        } else {
            RoomKind::Host
        },
    )
}

/// Records the room `room_id` of the streamer `owner_uid` as FLV.
///
/// The stream starts with the FLV header, which announces audio and video.
/// Each media connection must deliver a keyframe within 12 s, and then
/// video at least every 20 s; audio is written whenever it comes. When the
/// media drops, stalls or is refused, the stream reconnects up to 3 times,
/// 2 s apart, each time with a fresh ticket and room lookup, and continues
/// as the same FLV stream, its video at the next keyframe; 30 s of media
/// restore the reconnects. Packets that cannot be read are skipped.
///
/// It ends without an error when the room ends, and when a stream that once
/// had 30 s of media runs out of reconnects on a timeout: after a broadcast
/// ends, BIGO keeps answering the room lookup for about 45 s while its
/// servers send nothing. Otherwise it ends after its first error that is not
/// retried. An error before 30 s of media leaves the broadcaster to the web
/// stream for up to 30 minutes.
pub fn open(
    room_id: u64,
    owner_uid: u32,
    route: Route,
) -> impl Stream<Item = Result<FlvData, NativeError>> + Send + 'static {
    let recorder = Recorder::new(room_id, owner_uid, route);
    futures::stream::unfold(recorder, |mut recorder| async move {
        let item = recorder.next().await?;
        Some((item, recorder))
    })
}

struct Recorder {
    room_id: u64,
    owner_uid: u32,
    route: Route,
    muxer: Muxer,
    session: Option<Session>,
    /// Whether the stream has started. Until then the output is held, so
    /// that a failed start yields nothing.
    started: bool,
    held: Vec<FlvData>,
    output: VecDeque<FlvData>,
    /// Connection attempts, which pick the servers.
    connects: usize,
    reconnects: u32,
    /// Whether a connection has delivered media for [`HEALTHY_AFTER`].
    stable: bool,
    done: bool,
}

/// One pair of media connections.
struct Session {
    video: MediaConnection,
    audio: MediaConnection,
    depacketizer: VideoDepacketizer,
    startup: Startup,
    /// The muxer's video frame count when this session last wrote one.
    video_frames: u64,
    /// When this session fails unless it writes video.
    video_deadline: Instant,
    /// When this session will have delivered media long enough to restore
    /// the reconnects.
    healthy_at: Option<Instant>,
}

/// What a session waits for.
#[derive(Clone, Copy)]
enum Startup {
    /// A keyframe, by the video deadline.
    Keyframe,
    /// Audio, before the stream starts; without it the stream starts anyway.
    Audio(Instant),
    Done,
}

enum Received {
    Video(Bytes),
    Audio(Bytes),
}

/// How the stream goes on after an error.
#[derive(Debug)]
enum AfterError {
    Reconnect,
    End,
    Fail(NativeError),
}

impl Recorder {
    fn new(room_id: u64, owner_uid: u32, route: Route) -> Self {
        Self {
            room_id,
            owner_uid,
            route,
            muxer: Muxer::default(),
            session: None,
            started: false,
            held: Vec::new(),
            output: VecDeque::new(),
            connects: 0,
            reconnects: 0,
            stable: false,
            done: false,
        }
    }

    async fn next(&mut self) -> Option<Result<FlvData, NativeError>> {
        loop {
            if let Some(data) = self.output.pop_front() {
                return Some(Ok(data));
            }
            if self.done {
                return None;
            }
            let Err(error) = self.step().await else {
                continue;
            };
            self.session = None;
            match self.after_error(error) {
                AfterError::Reconnect => sleep(RECONNECT_DELAY).await,
                AfterError::End => self.done = true,
                AfterError::Fail(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
        }
    }

    fn after_error(&mut self, error: NativeError) -> AfterError {
        match error {
            NativeError::RoomEnded => {
                debug!(room_id = self.room_id, "room ended");
                AfterError::End
            }
            error if is_transient(&error) && self.reconnects < MAX_RECONNECTS => {
                self.reconnects += 1;
                warn!(
                    room_id = self.room_id,
                    %error,
                    attempt = self.reconnects,
                    "native stream interrupted; reconnecting"
                );
                if !self.started {
                    // Nothing went out yet, so start over cleanly.
                    self.muxer = Muxer::default();
                    self.held.clear();
                }
                AfterError::Reconnect
            }
            error @ NativeError::Timeout(_) if self.stable => {
                info!(
                    room_id = self.room_id,
                    %error,
                    "native stream out of reconnects; taking it as the end of the broadcast"
                );
                AfterError::End
            }
            error => {
                if !self.stable {
                    remember_failure(self.owner_uid);
                }
                AfterError::Fail(error)
            }
        }
    }

    /// Connects if needed, then handles one packet.
    async fn step(&mut self) -> Result<(), NativeError> {
        let session = match &mut self.session {
            Some(session) => session,
            None => {
                let session = self.connect().await?;
                self.session.insert(session)
            }
        };

        let deadline = match session.startup {
            Startup::Audio(at) => at.min(session.video_deadline),
            Startup::Keyframe | Startup::Done => session.video_deadline,
        };
        let received = tokio::select! {
            packet = session.video.next_packet() => Some(Received::Video(packet?)),
            packet = session.audio.next_packet() => Some(Received::Audio(packet?)),
            () = sleep_until(deadline) => None,
        };

        let mut tags = Vec::new();
        match received {
            None => match session.startup {
                Startup::Keyframe => return Err(NativeError::Timeout("a keyframe")),
                _ if Instant::now() >= session.video_deadline => {
                    return Err(NativeError::Timeout("video"));
                }
                _ => {
                    debug!(room_id = self.room_id, "no audio yet; starting without it");
                    session.startup = Startup::Done;
                }
            },
            Some(Received::Video(packet)) => {
                let mut frames = Vec::new();
                session.depacketizer.push(packet, &mut frames)?;
                for frame in frames {
                    self.muxer.push_video(frame, &mut tags)?;
                }
            }
            Some(Received::Audio(packet)) => {
                for frame in audio_frames(&packet, self.owner_uid)? {
                    self.muxer.push_audio(frame, &mut tags)?;
                }
            }
        }

        let now = Instant::now();
        if self.muxer.video_frames() > session.video_frames {
            session.video_frames = self.muxer.video_frames();
            session.video_deadline = now + VIDEO_TIMEOUT;
            // The muxer starts a session's video at a keyframe.
            if let Startup::Keyframe = session.startup {
                session.healthy_at = Some(now + HEALTHY_AFTER);
                session.startup = if self.started {
                    Startup::Done
                } else {
                    Startup::Audio(now + AUDIO_WAIT)
                };
            }
        }
        if let Startup::Audio(_) = session.startup
            && self.muxer.audio_frames() > 0
        {
            session.startup = Startup::Done;
        }
        if session.healthy_at.is_some_and(|at| now >= at) {
            session.healthy_at = None;
            self.reconnects = 0;
            if !self.stable {
                self.stable = true;
                forget_failure(self.owner_uid);
            }
        }
        if !self.started && matches!(session.startup, Startup::Done) {
            self.started = true;
            self.output.extend(self.held.drain(..));
        }
        if self.started {
            self.output.extend(tags);
        } else {
            self.held.extend(tags);
        }
        Ok(())
    }

    /// A fresh ticket and room lookup, then both media servers.
    async fn connect(&mut self) -> Result<Session, NativeError> {
        let ticket = ticket::guest(&self.route).await?;
        let channel = Linkd::connect(&self.route, &ticket)
            .await?
            .join_channel(self.room_id)
            .await?;
        // The media ticket is accepted for a few seconds from here. A server
        // that fails is not retried on this ticket; the next attempt rotates.
        let attempt = self.connects;
        self.connects += 1;
        let (Some(video_server), Some(audio_server)) = (
            pick(&channel.video_servers, attempt),
            pick(&channel.audio_servers, attempt),
        ) else {
            return Err(NativeError::Unsupported(
                "a room without TCP video or audio servers",
            ));
        };
        let (video, audio) = timeout(
            MEDIA_CONNECT_TIMEOUT,
            futures::future::try_join(
                MediaConnection::connect(
                    Track::Video,
                    video_server,
                    &channel,
                    self.owner_uid,
                    &self.route,
                ),
                MediaConnection::connect(
                    Track::Audio,
                    audio_server,
                    &channel,
                    self.owner_uid,
                    &self.route,
                ),
            ),
        )
        .await
        .map_err(|_| NativeError::Timeout("the media servers"))??;
        self.muxer.wait_for_keyframe();
        Ok(Session {
            video,
            audio,
            depacketizer: VideoDepacketizer::new(self.owner_uid),
            startup: Startup::Keyframe,
            video_frames: self.muxer.video_frames(),
            video_deadline: Instant::now() + KEYFRAME_TIMEOUT,
            healthy_at: None,
        })
    }
}

/// The server and port for connection attempt `attempt`: each server with a
/// TCP port in turn, then each with its next port.
fn pick(servers: &[MediaServer], attempt: usize) -> Option<(Ipv4Addr, u16)> {
    let servers: Vec<_> = servers
        .iter()
        .filter(|server| !server.tcp_ports.is_empty())
        .collect();
    let server = servers.get(attempt % servers.len().max(1))?;
    let round = attempt / servers.len();
    let port = server.tcp_ports.get(round % server.tcp_ports.len())?;
    Some((server.ip, *port))
}

/// Whether a fresh connection may get past `error`.
fn is_transient(error: &NativeError) -> bool {
    matches!(
        error,
        NativeError::Http(_)
            | NativeError::TicketRefused(_)
            | NativeError::Io(_)
            | NativeError::Timeout(_)
            | NativeError::Protocol(_)
            | NativeError::Rejected { .. }
    )
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufWriter;

    use ::flv::FlvWriter;
    use futures::StreamExt;
    use serde_json::Value;

    use super::*;
    use crate::extractor::default::{DEFAULT_UA, create_client_builder};

    /// A stream that once had 30 s of media ends without an error when its
    /// reconnects run out on timeouts, as after a broadcast ends. Before
    /// that, the same is a failure, remembered for the broadcaster.
    #[test]
    fn running_out_of_reconnects_ends_a_stable_stream() {
        // Broadcasters of their own: the memory is shared by the whole
        // process.
        const STABLE: u32 = 4_000_000_101;
        const STARTING: u32 = 4_000_000_102;
        let route = Route {
            client: reqwest::Client::new(),
            proxy: None,
        };
        let timeout = || NativeError::Timeout("a keyframe");

        let mut recorder = Recorder::new(1, STABLE, route.clone());
        recorder.stable = true;
        for _ in 0..MAX_RECONNECTS {
            let next = recorder.after_error(timeout());
            assert!(matches!(next, AfterError::Reconnect), "{next:?}");
        }
        let next = recorder.after_error(timeout());
        assert!(matches!(next, AfterError::End), "{next:?}");
        // Only a timeout: servers that refuse the connection still fail it.
        let refused = NativeError::Rejected {
            request: "media login",
            result: 1,
        };
        let next = recorder.after_error(refused);
        assert!(matches!(next, AfterError::Fail(_)), "{next:?}");
        assert!(!recently_failed(STABLE));

        let mut recorder = Recorder::new(2, STARTING, route);
        recorder.reconnects = MAX_RECONNECTS;
        let next = recorder.after_error(timeout());
        assert!(matches!(next, AfterError::Fail(_)), "{next:?}");
        assert!(recently_failed(STARTING));
    }

    /// Records 20 s of `room_id` to `path`, returning the keyframes, the
    /// audio tags and the last tag time.
    async fn record(
        room_id: u64,
        owner_uid: u32,
        route: Route,
        path: &std::path::Path,
    ) -> Result<(usize, usize, u32), NativeError> {
        let mut writer = FlvWriter::new(BufWriter::new(File::create(path)?))?;
        let mut stream = std::pin::pin!(open(room_id, owner_uid, route));
        let (mut keyframes, mut audio, mut last_time) = (0, 0, 0);
        while last_time < 20_000 {
            let Some(data) = stream.next().await else {
                return Err(NativeError::RoomEnded);
            };
            match data? {
                FlvData::Header(header) => writer.write_header(&header)?,
                FlvData::Tag(tag) => {
                    keyframes += usize::from(tag.is_key_frame_nalu());
                    audio += usize::from(tag.is_audio_tag());
                    last_time = last_time.max(tag.timestamp_ms);
                    writer.write_tag_f(&tag)?;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        writer.flush()?;
        Ok((keyframes, audio, last_time))
    }

    /// Records 20 s of a live room to `BIGO_FLV_OUT` (default
    /// `bigo-native.flv` in the temp directory), to check by hand with
    /// ffprobe. `BIGO_ROOM=<room id>:<owner uid>` picks the room; otherwise
    /// it is the first single-host room of the public live list that
    /// records.
    #[tokio::test]
    #[ignore = "records a live BIGO room"]
    async fn records_a_live_room() {
        // Shows reconnects and substream switches.
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_test_writer()
            .try_init();
        // Direct, so that the ticket and the media servers see the same
        // address.
        let client = create_client_builder(None).no_proxy().build().unwrap();
        let route = Route {
            client,
            proxy: None,
        };
        let rooms: Vec<(u64, u32)> = match std::env::var("BIGO_ROOM") {
            Ok(room) => {
                let (room_id, owner_uid) = room.split_once(':').unwrap();
                vec![(room_id.parse().unwrap(), owner_uid.parse().unwrap())]
            }
            Err(_) => {
                let list: Value = route
                    .client
                    .get("https://ta.bigo.tv/official_website/OInterfaceWeb/vedioList/72")
                    .header(reqwest::header::USER_AGENT, DEFAULT_UA)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                list["data"]["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|room| {
                        Some((room["room_id"].as_u64()?, room["owner"].as_u64()? as u32))
                    })
                    .collect()
            }
        };
        let path = std::env::var("BIGO_FLV_OUT")
            .map_or_else(|_| std::env::temp_dir().join("bigo-native.flv"), Into::into);
        for (room_id, owner_uid) in rooms.into_iter().take(5) {
            match probe(room_id, &route).await {
                Ok(RoomKind::Host) => {}
                result => {
                    eprintln!("room {room_id}: {result:?}");
                    continue;
                }
            }
            // A failing room spends its reconnects, which takes under 2 min.
            let recorded = timeout(
                Duration::from_secs(180),
                record(room_id, owner_uid, route.clone(), &path),
            )
            .await
            .unwrap_or(Err(NativeError::Timeout("20 s of media")));
            match recorded {
                Ok((keyframes, audio, last_time)) => {
                    eprintln!(
                        "room {room_id}: wrote {}: {keyframes} keyframes, {audio} audio tags, \
                         {last_time} ms",
                        path.display()
                    );
                    // A room can have no audio.
                    assert!(keyframes >= 5);
                    return;
                }
                Err(error) => eprintln!("room {room_id}: {error}"),
            }
        }
        panic!("no listed room recorded");
    }
}
