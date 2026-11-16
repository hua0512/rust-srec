//! BIGO's media servers: one unencrypted TCP connection per track, admitted
//! by the media ticket of a [`Channel`].
//!
//! The servers send two kinds of packet. Media arrives packed: a u32 word
//! with bit 31 set holds the packet's size (bits 4–15, counting the word)
//! and its kind (bits 0–3). Control messages are frames as on Linkd,
//! `[len u32][uri u32][status u16][body]`, and some of them ask for an
//! answer that keeps the stream going.

use std::io;
use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, sleep_until, timeout};
use tracing::debug;

use super::linkd::{Channel, STATUS_OK, frame_len, put_blob, put_frame};
use super::{NativeError, Route};
use crate::danmaku::proxy::BoxedTransport;

const VIDEO_LOGIN: u32 = 0x37902;
const AUDIO_LOGIN: u32 = 0x32302;
const LOGIN_ANSWER: u32 = 0x32402;
const KEEPALIVE: u32 = 0x32e02;
// The server's messages that the app answers by echoing parts of them. Of
// their content only the video stream count in `STREAMS` is used.
const PING: u32 = 0x114_2f02;
const PING_ANSWER: u32 = 0x114_3002;
const STREAMS: u32 = 0x36b702;
const STREAMS_ANSWER: u32 = 0x36b802;
const STREAM_STATE: u32 = 0x36b502;
const STREAM_STATE_ANSWER: u32 = 0x36b602;
const SUBSCRIBE: u32 = 0x5902;
const SUBSCRIBED: u32 = 0x5a02;

/// The kinds of packed packet that carry each track's media.
const VIDEO_PACKET: u32 = 5;
const AUDIO_PACKET: u32 = 2;

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
/// A track counts as dead after this long without media. Audio only once
/// the connection has delivered some: a room can have no audio at all.
const MEDIA_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound on the server accepting what is queued for it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Which track a connection carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Track {
    Video,
    Audio,
}

/// A logged-in connection to one track's media server.
pub(super) struct MediaConnection {
    io: BoxedTransport,
    track: Track,
    source: u32,
    sid: u32,
    timestamp: u32,
    input: BytesMut,
    output: BytesMut,
    keepalive_at: Instant,
    media_deadline: Option<Instant>,
    /// The id of the video subscription once sent.
    subscription: Option<u64>,
    /// Whether the server confirmed the subscription. Video before that is
    /// dropped.
    subscribed: bool,
}

/// A packet as received.
enum Packet {
    Media { kind: u32, data: Bytes },
    Control { uri: u32, status: u16, body: Bytes },
}

impl MediaConnection {
    /// Connects to `server` and sends the login with `channel`'s media
    /// ticket. `owner_uid` is the streamer whose video the login asks for.
    pub(super) async fn connect(
        track: Track,
        (ip, port): (Ipv4Addr, u16),
        channel: &Channel,
        owner_uid: u32,
        route: &Route,
    ) -> Result<Self, NativeError> {
        let io = route.open(&ip.to_string(), port).await?;
        let now = Instant::now();
        let mut connection = Self {
            io,
            track,
            source: channel.source,
            sid: channel.sid,
            timestamp: channel.timestamp,
            input: BytesMut::new(),
            output: BytesMut::new(),
            keepalive_at: now + KEEPALIVE_INTERVAL,
            media_deadline: (track == Track::Video).then_some(now + MEDIA_TIMEOUT),
            subscription: None,
            subscribed: false,
        };
        let (uri, body) = match track {
            Track::Video => (VIDEO_LOGIN, video_login_body(channel, owner_uid, unix_ms())),
            Track::Audio => (AUDIO_LOGIN, audio_login_body(channel)),
        };
        put_frame(&mut connection.output, uri, &body);
        connection.flush().await?;
        debug!(?track, %ip, port, "connected to a media server");
        Ok(connection)
    }

    /// The next media packet of this connection's track, from its first
    /// byte. Meanwhile it answers control messages and sends keepalives, so
    /// the stream only flows while this is awaited.
    ///
    /// Cancel-safe.
    pub(super) async fn next_packet(&mut self) -> Result<Bytes, NativeError> {
        loop {
            let now = Instant::now();
            if now >= self.keepalive_at {
                let body = keepalive_body(self.source, self.sid, self.timestamp, unix_ms());
                put_frame(&mut self.output, KEEPALIVE, &body);
                self.keepalive_at = now + KEEPALIVE_INTERVAL;
            }
            timeout(WRITE_TIMEOUT, self.flush())
                .await
                .map_err(|_| NativeError::Timeout("a media server to accept data"))??;

            let keepalive_at = self.keepalive_at;
            let media_deadline = self.media_deadline;
            let packet = tokio::select! {
                packet = self.read_packet() => packet?,
                () = sleep_until(keepalive_at) => continue,
                () = sleep_until(media_deadline.unwrap_or(keepalive_at)), if media_deadline.is_some() => {
                    return Err(NativeError::Timeout(match self.track {
                        Track::Video => "video",
                        Track::Audio => "audio",
                    }));
                }
            };
            match packet {
                Packet::Media { kind, data } => {
                    let wanted = match self.track {
                        Track::Video => kind == VIDEO_PACKET && self.subscribed,
                        Track::Audio => kind == AUDIO_PACKET,
                    };
                    if wanted {
                        self.media_deadline = Some(Instant::now() + MEDIA_TIMEOUT);
                        return Ok(data);
                    }
                }
                Packet::Control { uri, status, body } => self.control(uri, status, &body)?,
            }
        }
    }

    /// Answers a control message.
    fn control(&mut self, uri: u32, status: u16, body: &[u8]) -> Result<(), NativeError> {
        if status != STATUS_OK {
            // This ends the connection, a refused login included; the caller
            // then looks the room up again for a fresh media ticket.
            return Err(NativeError::Rejected {
                request: if uri == LOGIN_ANSWER {
                    "media login"
                } else {
                    "media connection"
                },
                result: i32::from(status),
            });
        }
        let short = || NativeError::Protocol(format!("media control message {uri:#x} is short"));
        match uri {
            LOGIN_ANSWER => debug!(track = ?self.track, "media login answered"),
            PING => {
                let (Some(head), Some(tail)) = (body.get(..8), body.get(16..28)) else {
                    return Err(short());
                };
                put_frame(&mut self.output, PING_ANSWER, &[head, tail].concat());
            }
            STREAMS => {
                put_frame(
                    &mut self.output,
                    STREAMS_ANSWER,
                    body.get(..16).ok_or_else(short)?,
                );
                let streams = body.get(16..20).ok_or_else(short)?;
                if self.track == Track::Video && self.subscription.is_none() && streams != [0; 4] {
                    let id = unix_ms();
                    put_frame(
                        &mut self.output,
                        SUBSCRIBE,
                        &subscribe_body(self.source, self.sid, id),
                    );
                    self.subscription = Some(id);
                }
            }
            SUBSCRIBED => {
                let mut body = body;
                if body.try_get_u32_le()? == self.source
                    && Some(body.try_get_u64_le()?) == self.subscription
                {
                    debug!("video subscription confirmed");
                    self.subscribed = true;
                }
            }
            // Shorter ones need no answer.
            STREAM_STATE => {
                if let Some(head) = body.get(..16) {
                    put_frame(&mut self.output, STREAM_STATE_ANSWER, head);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Writes the queued messages. Cancel-safe.
    async fn flush(&mut self) -> Result<(), NativeError> {
        while !self.output.is_empty() {
            if self.io.write_buf(&mut self.output).await? == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            }
        }
        self.io.flush().await?;
        Ok(())
    }

    /// The next packet. Cancel-safe: bytes stay buffered until their packet
    /// is complete.
    async fn read_packet(&mut self) -> Result<Packet, NativeError> {
        loop {
            if let Some(packet) = self.take_packet()? {
                return Ok(packet);
            }
            self.input.reserve(64 * 1024);
            if self.io.read_buf(&mut self.input).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the media server closed the connection",
                )
                .into());
            }
        }
    }

    fn take_packet(&mut self) -> Result<Option<Packet>, NativeError> {
        let Ok(word) = (&self.input[..]).try_get_u32_le() else {
            return Ok(None);
        };
        if word & 0x8000_0000 != 0 {
            let size = (word >> 4 & 0xfff) as usize;
            if size < 4 {
                return Err(NativeError::Protocol(format!("media packet size {size}")));
            }
            if self.input.len() < size {
                return Ok(None);
            }
            let data = self.input.split_to(size).freeze();
            return Ok(Some(Packet::Media {
                kind: word & 0xf,
                data,
            }));
        }
        let len = frame_len(word)?;
        if self.input.len() < len {
            return Ok(None);
        }
        let mut frame = self.input.split_to(len).freeze();
        frame.advance(4);
        let uri = frame.try_get_u32_le()?;
        let status = frame.try_get_u16_le()?;
        Ok(Some(Packet::Control {
            uri,
            status,
            body: frame,
        }))
    }
}

/// The login's common prefix: who logs in where, with the media ticket.
fn login_prefix(channel: &Channel) -> Vec<u8> {
    let mut body = Vec::with_capacity(256);
    body.put_u32_le(channel.source);
    body.put_u32_le(channel.sid);
    body.put_u32_le(0);
    put_blob(&mut body, &channel.media_ticket);
    body.put_u32_le(channel.timestamp);
    body
}

/// The video login. Besides the channel and the streamer, it carries fixed
/// values as the app sends them, including its version.
fn video_login_body(channel: &Channel, owner_uid: u32, now_ms: u64) -> Vec<u8> {
    let mut body = login_prefix(channel);
    body.put_u32_le(0x94);
    body.put_u32_le(owner_uid);
    body.put_u64_le(now_ms);
    body.put_u8(1);
    put_bytes(&mut body, &[]);
    put_bytes(&mut body, &[0]);
    body.put_u8(0);
    for _ in 0..3 {
        put_blob(&mut body, &[]);
    }
    body.put_u32_le(10);
    body.put_u32_le(1);
    body.put_slice(&[0x8e, 0, 0, 1]);
    body.put_u32_le(0);
    body.put_u8(0x17);
    put_bytes(&mut body, &[0, 1, 2, 3, 4]);
    // A map of two strings.
    body.put_u32_le(2);
    for value in ["client_version", "6.53.0", "os", "0"] {
        put_blob(&mut body, value.as_bytes());
    }
    body.put_u8(3);
    body.put_u8(0);
    body
}

/// The audio login, with fixed values as the app sends them.
fn audio_login_body(channel: &Channel) -> Vec<u8> {
    let mut body = login_prefix(channel);
    body.put_u32_le(0x9b7);
    body.put_u8(0);
    body.put_bytes(0, 12);
    put_bytes(&mut body, &[]);
    put_bytes(&mut body, &[]);
    body.put_u32_le(0);
    body.put_u8(0);
    put_blob(&mut body, &[]);
    body.put_u32_le(0);
    put_blob(&mut body, &[]);
    put_blob(&mut body, &[]);
    body.put_bytes(0, 20);
    body.put_u8(0);
    body
}

/// Subscribes the viewer `source` to the room's video. `id` comes back in
/// the confirmation.
fn subscribe_body(source: u32, sid: u32, id: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(72);
    body.put_u32_le(source);
    body.put_u32_le(1);
    body.put_u64_le(id);
    put_bytes(&mut body, &[]);
    put_bytes(&mut body, &[]);
    put_bytes(&mut body, &[0]);
    body.put_u8(0x98);
    body.put_u32_le(sid);
    body.put_u8(0);
    body.put_u8(0);
    body.put_bytes(0, 32);
    // The app ends with 5, 3, 0. After that ending the server sometimes
    // moved a room to a lower H.264 rendition at once; after 5, 0, 0 it was
    // never seen to.
    body.put_slice(&[5, 0, 0]);
    body
}

fn keepalive_body(source: u32, sid: u32, timestamp: u32, now_ms: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(22);
    body.put_u32_le(source);
    body.put_u32_le(sid);
    // The clock's low 32 bits.
    body.put_u32_le(now_ms as u32);
    body.put_u32_le(timestamp);
    body.put_u32_le(0);
    put_blob(&mut body, &[]);
    body
}

/// A u32-length-prefixed byte string.
fn put_bytes(out: &mut impl BufMut, data: &[u8]) {
    out.put_u32_le(data.len() as u32);
    out.put_slice(data);
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}
