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
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, Sleep, sleep_until, timeout};
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
/// Audio counts as dead after this long without any, once the connection
/// has delivered some: a room can have no audio at all. Video is watched by
/// the recorder, which sees whether it yields frames.
const AUDIO_TIMEOUT: Duration = Duration::from_secs(20);
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
    /// Bytes read from the server. Packets are copied out of it rather than
    /// split off: a split-off packet would keep its whole 64 KiB read buffer
    /// alive while anything downstream holds it, such as a fragment of an
    /// incomplete frame or audio held for the first keyframe, and each read
    /// meanwhile would need a new buffer.
    input: BytesMut,
    output: BytesMut,
    /// When the next keepalive is due. The timers last as long as the
    /// connection, so that a packet costs no new timer.
    keepalive: Pin<Box<Sleep>>,
    /// When audio counts as dead: armed by the first audio, and pushed back
    /// by each.
    audio_timeout: Pin<Box<Sleep>>,
    audio_armed: bool,
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
        let mut connection = Self::new(io, track, channel.source, channel.sid, channel.timestamp);
        let (uri, body) = match track {
            Track::Video => (VIDEO_LOGIN, video_login_body(channel, owner_uid, unix_ms())),
            Track::Audio => (AUDIO_LOGIN, audio_login_body(channel)),
        };
        put_frame(&mut connection.output, uri, &body);
        connection.flush().await?;
        debug!(?track, %ip, port, "connected to a media server");
        Ok(connection)
    }

    /// A connection over `io`, before its login.
    fn new(io: BoxedTransport, track: Track, source: u32, sid: u32, timestamp: u32) -> Self {
        let now = Instant::now();
        Self {
            io,
            track,
            source,
            sid,
            timestamp,
            input: BytesMut::new(),
            output: BytesMut::new(),
            keepalive: Box::pin(sleep_until(now + KEEPALIVE_INTERVAL)),
            audio_timeout: Box::pin(sleep_until(now + AUDIO_TIMEOUT)),
            audio_armed: false,
            subscription: None,
            subscribed: false,
        }
    }

    /// The next media packet of this connection's track, from its first
    /// byte. Meanwhile it answers control messages and sends keepalives, so
    /// the stream only flows while this is awaited.
    ///
    /// Cancel-safe.
    pub(super) async fn next_packet(&mut self) -> Result<Bytes, NativeError> {
        loop {
            if !self.output.is_empty() {
                timeout(WRITE_TIMEOUT, self.flush())
                    .await
                    .map_err(|_| NativeError::Timeout("a media server to accept data"))??;
            }

            // The timers first, so that packets arriving back to back cannot
            // hold them up.
            let packet = tokio::select! {
                biased;
                () = &mut self.keepalive => {
                    let body = keepalive_body(self.source, self.sid, self.timestamp, unix_ms());
                    put_frame(&mut self.output, KEEPALIVE, &body);
                    self.keepalive
                        .as_mut()
                        .reset(Instant::now() + KEEPALIVE_INTERVAL);
                    continue;
                }
                () = &mut self.audio_timeout, if self.audio_armed => {
                    return Err(NativeError::Timeout("audio"));
                }
                packet = Self::read_packet(&mut self.io, &mut self.input) => packet?,
            };
            match packet {
                Packet::Media { kind, data } => match self.track {
                    Track::Video if kind == VIDEO_PACKET && self.subscribed => return Ok(data),
                    Track::Audio if kind == AUDIO_PACKET => {
                        self.audio_timeout
                            .as_mut()
                            .reset(Instant::now() + AUDIO_TIMEOUT);
                        self.audio_armed = true;
                        return Ok(data);
                    }
                    _ => {}
                },
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
        let ignored = || {
            debug!(
                track = ?self.track,
                uri,
                len = body.len(),
                "short media control message ignored"
            );
        };
        match uri {
            LOGIN_ANSWER => debug!(track = ?self.track, "media login answered"),
            PING => match (body.get(..8), body.get(16..28)) {
                (Some(head), Some(tail)) => {
                    put_frame(&mut self.output, PING_ANSWER, &[head, tail].concat());
                }
                _ => ignored(),
            },
            STREAMS => {
                put_frame(
                    &mut self.output,
                    STREAMS_ANSWER,
                    body.get(..16).ok_or_else(short)?,
                );
                if self.track == Track::Video
                    && self.subscription.is_none()
                    && body.get(16..20).ok_or_else(short)? != [0; 4]
                {
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
                match (body.try_get_u32_le(), body.try_get_u64_le()) {
                    (Ok(source), Ok(id)) => {
                        if source == self.source && Some(id) == self.subscription {
                            debug!("video subscription confirmed");
                            self.subscribed = true;
                        }
                    }
                    _ => ignored(),
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

    /// The next packet from `io`. Cancel-safe: bytes stay buffered in `input`
    /// until their packet is complete. It borrows only these two fields, so
    /// that [`Self::next_packet`] can wait on its timers meanwhile.
    async fn read_packet(
        io: &mut BoxedTransport,
        input: &mut BytesMut,
    ) -> Result<Packet, NativeError> {
        loop {
            if let Some(packet) = Self::take_packet(input)? {
                return Ok(packet);
            }
            input.reserve(64 * 1024);
            if io.read_buf(input).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the media server closed the connection",
                )
                .into());
            }
        }
    }

    fn take_packet(input: &mut BytesMut) -> Result<Option<Packet>, NativeError> {
        let Ok(word) = (&input[..]).try_get_u32_le() else {
            return Ok(None);
        };
        if word & 0x8000_0000 != 0 {
            let size = (word >> 4 & 0xfff) as usize;
            if size < 4 {
                return Err(NativeError::Protocol(format!("media packet size {size}")));
            }
            if input.len() < size {
                return Ok(None);
            }
            let data = Bytes::copy_from_slice(&input[..size]);
            input.advance(size);
            return Ok(Some(Packet::Media {
                kind: word & 0xf,
                data,
            }));
        }
        let len = frame_len(word)?;
        if input.len() < len {
            return Ok(None);
        }
        let mut frame = Bytes::copy_from_slice(&input[..len]);
        input.advance(len);
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

#[cfg(test)]
impl MediaConnection {
    /// A connection over `io` whose login and video subscription are done.
    pub(super) fn logged_in(io: BoxedTransport, track: Track) -> Self {
        let mut connection = Self::new(io, track, 1, 2, 3);
        connection.subscribed = true;
        connection
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::sleep;

    use super::*;

    /// The timers outlive the packets: keepalives go out every 5 s, and
    /// audio times out 20 s after its last packet.
    #[tokio::test(start_paused = true)]
    async fn keepalives_repeat_and_audio_times_out_after_its_last_packet() {
        // Room for the keepalives expected, but not for a flood of them.
        let (io, server) = tokio::io::duplex(1024);
        let (mut written, mut to_client) = tokio::io::split(server);
        // A packed audio packet, whose content is not read here.
        let packet = [
            (0x8000_0000 | (8 << 4) | AUDIO_PACKET).to_le_bytes(),
            [0; 4],
        ]
        .concat();
        tokio::spawn(async move {
            to_client.write_all(&packet).await.unwrap();
            sleep(Duration::from_secs(12)).await;
            to_client.write_all(&packet).await.unwrap();
            // Keeps the connection open.
            std::future::pending::<()>().await;
        });
        let start = Instant::now();
        let mut connection = MediaConnection::logged_in(Box::new(io), Track::Audio);

        connection.next_packet().await.unwrap();
        connection.next_packet().await.unwrap();
        assert_eq!(start.elapsed().as_secs(), 12);
        let error = connection.next_packet().await.unwrap_err();
        assert!(matches!(error, NativeError::Timeout("audio")), "{error}");
        assert_eq!(start.elapsed().as_secs(), 32);

        // Closed, so that reading what it wrote cannot wait for more.
        drop(connection);
        let mut frames = Vec::new();
        written.read_to_end(&mut frames).await.unwrap();
        let mut frames = &frames[..];
        let mut keepalives = 0;
        while let Ok(len) = frames.try_get_u32_le() {
            keepalives += usize::from(frames.get_u32_le() == KEEPALIVE);
            frames.advance(len as usize - 8);
        }
        // At 5, 10, ..., 30 s.
        assert_eq!(keepalives, 6);
    }
}
