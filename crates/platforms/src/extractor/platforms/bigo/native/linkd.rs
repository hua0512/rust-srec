//! Linkd, BIGO's app service: an encrypted TCP connection the app signs in
//! on. A guest login looks up a room's media servers
//! ([`Linkd::join_channel`]); the listener keeps a signed-in one open.
//!
//! Every frame is `[len u32][uri u32][status u16][body]`, little-endian, with
//! `len` counting the whole frame. A connection starts with a key exchange:
//! the client sends an ephemeral RSA-512 public key, and the server answers
//! with an RC4 session key encrypted to it. Each direction then runs its own
//! RC4 keystream from that key.

use std::fmt;
use std::io;
use std::net::Ipv4Addr;
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};
use num_bigint::BigUint;
use rand::RngExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tracing::debug;

use super::ticket::Ticket;
use super::{NativeError, Route};
use crate::danmaku::proxy::BoxedTransport;

/// Linkd's addresses, tried in order. The ticket answer lists none.
const ADDRESSES: [(&str, u16); 2] = [("164.90.67.6", 54953), ("164.90.64.60", 1299)];
/// Bound on reaching one address, exchanging keys and signing in.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound on the answer to a room lookup.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// A frame's length, uri and status.
const HEADER_LEN: usize = 10;
/// Largest frame accepted; Linkd's frames are a few hundred bytes.
const MAX_FRAME_LEN: usize = 1 << 20;
/// The status requests carry and successful answers report.
pub(super) const STATUS_OK: u16 = 200;

const KEY_REQUEST: u32 = 5636;
const KEY_ANSWER: u32 = 5892;
const PREPARE_LOGIN: u32 = 512_279;
const PREPARE_LOGIN_ANSWER: u32 = 512_535;
const LOGIN: u32 = 512_791;
const LOGIN_ANSWER: u32 = 513_047;
const JOIN_CHANNEL: u32 = 5064;
const JOIN_CHANNEL_ANSWER: u32 = 1224;

/// Prepare-login result for a refused native login; see
/// [`NativeError::IpRestricted`].
const IP_RESTRICTED: i32 = 10;
/// The app state prepare-login reports.
const FOREGROUND: i16 = 0;
/// The client type prepare-login reports.
const CLIENT_TYPE: i8 = 1;
/// The `PJoinChannel2` room flag of a room with several hosts.
const MULTI_GUEST: u32 = 4;

const RSA_EXPONENT: u32 = 65_537;
/// Bytes in an RSA-512 modulus, and so in a decrypted block.
const MODULUS_LEN: usize = 64;
const PRIME_LEN: usize = MODULUS_LEN / 2;
const MILLER_RABIN_ROUNDS: usize = 32;
const SMALL_PRIMES: [u32; 15] = [3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53];

/// A signed-in Linkd connection.
pub struct Linkd {
    io: BoxedTransport,
    send: Rc4,
    recv: Rc4,
    /// Decrypted bytes not yet returned as frames.
    input: BytesMut,
    /// Encrypted bytes not yet written.
    output: BytesMut,
    /// The signed-in uid.
    pub(super) uid: u32,
}

/// A frame as received.
pub(super) struct Frame {
    pub(super) uri: u32,
    /// [`STATUS_OK`] unless the request it answers failed.
    pub(super) status: u16,
    pub(super) body: Bytes,
}

impl Linkd {
    /// Signs in with `ticket` at the first address that accepts it. An
    /// address that is unreachable, times out or answers
    /// [`NativeError::IpRestricted`] moves on to the next; a rejected login
    /// or an unexpected answer ends the attempt. When one address answered
    /// `IpRestricted` and the rest could not be reached, that is the error.
    pub async fn connect(route: &Route, ticket: &Ticket) -> Result<Self, NativeError> {
        let [first, others @ ..] = ADDRESSES;
        let mut result = Self::connect_to(first, route, ticket).await;
        let mut restricted = false;
        for address in others {
            match result {
                Err(NativeError::IpRestricted) => restricted = true,
                Err(NativeError::Io(_) | NativeError::Timeout(_)) => {}
                _ => break,
            }
            result = Self::connect_to(address, route, ticket).await;
        }
        match result {
            Err(NativeError::Io(_) | NativeError::Timeout(_)) if restricted => {
                Err(NativeError::IpRestricted)
            }
            result => result,
        }
    }

    async fn connect_to(
        (host, port): (&str, u16),
        route: &Route,
        ticket: &Ticket,
    ) -> Result<Self, NativeError> {
        let result = timeout(LOGIN_TIMEOUT, Self::log_in(host, port, route, ticket))
            .await
            .unwrap_or(Err(NativeError::Timeout("the Linkd login")));
        match &result {
            Ok(_) => debug!(host, port, uid = ticket.uid, "signed in to Linkd"),
            Err(error) => debug!(host, port, %error, "Linkd login failed"),
        }
        result
    }

    async fn log_in(
        host: &str,
        port: u16,
        route: &Route,
        ticket: &Ticket,
    ) -> Result<Self, NativeError> {
        let mut io = route.open(host, port).await?;
        let key = exchange_keys(&mut io).await?;
        let mut linkd = Self {
            io,
            send: Rc4::new(&key),
            recv: Rc4::new(&key),
            input: BytesMut::new(),
            output: BytesMut::new(),
            uid: ticket.uid,
        };

        linkd.queue(PREPARE_LOGIN, &prepare_login_body(ticket));
        let mut answer = &linkd.answer(PREPARE_LOGIN_ANSWER, |_| true).await?[..];
        // The uid, then the result.
        answer.try_get_u32_le()?;
        match answer.try_get_i32_le()? {
            200 => {}
            IP_RESTRICTED => return Err(NativeError::IpRestricted),
            result => {
                return Err(NativeError::Rejected {
                    request: "prepare-login",
                    result,
                });
            }
        }

        let mut body = Vec::new();
        put_blob(&mut body, b"CN");
        linkd.queue(LOGIN, &body);
        let mut answer = &linkd.answer(LOGIN_ANSWER, |_| true).await?[..];
        match answer.try_get_i32_le()? {
            0 | 200 => Ok(linkd),
            result => Err(NativeError::Rejected {
                request: "login",
                result,
            }),
        }
    }

    /// Looks up a room's media servers, then closes the connection. The
    /// answer's media ticket must be used within a few seconds.
    pub async fn join_channel(mut self, room_id: u64) -> Result<Channel, NativeError> {
        let seq = rand::rng().random::<u32>();
        // The room's session id is the low 32 bits of its id.
        let sid = room_id as u32;
        self.queue(JOIN_CHANNEL, &join_channel_body(seq, self.uid, sid));
        // The answer's seq follows its u16 code.
        let lookup = self.answer(JOIN_CHANNEL_ANSWER, |body| {
            body.get(2..6) == Some(&seq.to_le_bytes()[..])
        });
        let answer = timeout(ANSWER_TIMEOUT, lookup)
            .await
            .map_err(|_| NativeError::Timeout("the room lookup"))??;
        let channel = decode_channel(&answer);
        match &channel {
            Ok(channel) => debug!(room_id, ?channel, "Linkd room lookup answered"),
            Err(error) => debug!(room_id, %error, "Linkd room lookup failed"),
        }
        channel
    }

    /// Writes the queued requests, then reads frames until one with `uri`
    /// whose body `matches` arrives, dropping the others.
    async fn answer(
        &mut self,
        uri: u32,
        matches: impl Fn(&[u8]) -> bool,
    ) -> Result<Bytes, NativeError> {
        self.flush().await?;
        loop {
            let frame = self.read_frame().await?;
            if frame.uri == uri && matches(&frame.body) {
                return Ok(frame.body);
            }
        }
    }

    /// Encrypts a frame onto the output; [`flush`](Self::flush) writes it.
    pub(super) fn queue(&mut self, uri: u32, body: &[u8]) {
        let start = self.output.len();
        put_frame(&mut self.output, uri, body);
        self.send.apply(&mut self.output[start..]);
    }

    /// Writes the queued frames. Cancel-safe: what is not yet written stays
    /// queued.
    pub(super) async fn flush(&mut self) -> Result<(), NativeError> {
        while !self.output.is_empty() {
            if self.io.write_buf(&mut self.output).await? == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            }
        }
        self.io.flush().await?;
        Ok(())
    }

    /// The next frame. Cancel-safe: bytes are decrypted once, as they
    /// arrive, and kept until their frame is complete.
    pub(super) async fn read_frame(&mut self) -> Result<Frame, NativeError> {
        loop {
            if let Some(frame) = self.take_frame()? {
                return Ok(frame);
            }
            let start = self.input.len();
            self.input.reserve(4096);
            if self.io.read_buf(&mut self.input).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Linkd closed the connection",
                )
                .into());
            }
            self.recv.apply(&mut self.input[start..]);
        }
    }

    fn take_frame(&mut self) -> Result<Option<Frame>, NativeError> {
        let Ok(len) = (&self.input[..]).try_get_u32_le() else {
            return Ok(None);
        };
        let len = frame_len(len)?;
        if self.input.len() < len {
            return Ok(None);
        }
        let mut frame = self.input.split_to(len).freeze();
        frame.advance(4);
        let uri = frame.try_get_u32_le()?;
        let status = frame.try_get_u16_le()?;
        Ok(Some(Frame {
            uri,
            status,
            body: frame,
        }))
    }
}

/// A room's media servers and the ticket that admits this login to them.
pub struct Channel {
    /// The viewer's id towards the media servers.
    pub source: u32,
    /// The room's session id: the low 32 bits of its id.
    pub sid: u32,
    /// Admits the viewer to the media servers; accepted only for a few seconds.
    pub media_ticket: Bytes,
    /// Goes to the media servers along with the ticket.
    pub timestamp: u32,
    /// Servers for the audio track (BIGO's "media" servers).
    pub audio_servers: Vec<MediaServer>,
    /// Servers for the video track.
    pub video_servers: Vec<MediaServer>,
    /// The room flags in the answer's optional suffix.
    flags: Option<u32>,
}

impl Channel {
    /// Whether several hosts share the room.
    pub fn multi_guest(&self) -> bool {
        self.flags.is_some_and(|flags| flags & MULTI_GUEST != 0)
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("source", &self.source)
            .field("sid", &self.sid)
            .field("timestamp", &self.timestamp)
            .field("audio_servers", &self.audio_servers)
            .field("video_servers", &self.video_servers)
            .field("flags", &self.flags)
            .finish_non_exhaustive()
    }
}

/// A media server and its TCP ports.
#[derive(Clone, Debug)]
pub struct MediaServer {
    pub ip: Ipv4Addr,
    pub tcp_ports: Vec<u16>,
}

/// Sends the client's key and returns the session key from the server's
/// answer.
async fn exchange_keys(io: &mut BoxedTransport) -> Result<Vec<u8>, NativeError> {
    let key = RsaKey::generate();
    let hello = {
        let mut rng = rand::rng();
        let mut seed = [0u8; 4];
        rng.fill(&mut seed);
        seed[3] = seed[3] & 0x0f | 0x40;
        let mut padding = vec![0u8; rng.random_range(0..4)];
        rng.fill(&mut padding[..]);

        let mut body = Vec::new();
        put_blob(&mut body, &java_bytes(&key.n));
        put_blob(&mut body, &java_bytes(&BigUint::from(RSA_EXPONENT)));
        put_blob(&mut body, &padding);
        let mut hello = seed.to_vec();
        put_frame(&mut hello, KEY_REQUEST, &body);
        Rc4::new(&seed).apply(&mut hello[seed.len()..]);
        hello
    };
    io.write_all(&hello).await?;
    io.flush().await?;

    // The answer is a seed and one frame encrypted with it. Exact reads keep
    // the bytes after it, which use the session key, unread.
    let mut seed = [0u8; 4];
    io.read_exact(&mut seed).await?;
    let mut cipher = Rc4::new(&seed);
    let mut len = [0u8; 4];
    io.read_exact(&mut len).await?;
    cipher.apply(&mut len);
    // The rest of the frame, after its length.
    let mut frame = vec![0u8; frame_len(u32::from_le_bytes(len))? - len.len()];
    io.read_exact(&mut frame).await?;
    cipher.apply(&mut frame);

    let mut frame = &frame[..];
    let uri = frame.try_get_u32_le()?;
    let status = frame.try_get_u16_le()?;
    if uri != KEY_ANSWER || status != STATUS_OK {
        return Err(NativeError::Protocol(format!(
            "key exchange answered uri {uri} status {status}"
        )));
    }
    key.decrypt(get_blob(&mut frame)?)
}

pub(super) fn frame_len(len: u32) -> Result<usize, NativeError> {
    let len = len as usize;
    if (HEADER_LEN..=MAX_FRAME_LEN).contains(&len) {
        Ok(len)
    } else {
        Err(NativeError::Protocol(format!("frame length {len}")))
    }
}

fn prepare_login_body(ticket: &Ticket) -> Vec<u8> {
    let mut body = Vec::with_capacity(160);
    body.put_u32_le(ticket.uid);
    put_blob(&mut body, &ticket.token);
    put_blob(&mut body, b"0");
    put_blob(&mut body, ticket.user_name.as_bytes());
    put_blob(&mut body, ticket.device_id.as_bytes());
    body.put_i32_le(0);
    body.put_i16_le(FOREGROUND);
    body.put_i16_le(0);
    body.put_i32_le(0);
    body.put_i8(0);
    body.put_i32_le(0);
    body.put_i16_le(0);
    body.put_i8(0);
    body.put_i16_le(0);
    body.put_i8(CLIENT_TYPE);
    body.put_i16_le(0);
    body.put_i32_le(0);
    body.put_i16_le(0);
    body.put_i32_le(0);
    for value in ["", "", "CN", ""] {
        put_blob(&mut body, value.as_bytes());
    }
    body.put_u32_le(0);
    // An empty map.
    body.put_u32_le(0);
    body
}

/// `PJoinChannel2`. Besides the seq, viewer and room, it carries fixed
/// values as the app sends them.
fn join_channel_body(seq: u32, viewer: u32, sid: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(31);
    body.put_u32_le(seq);
    body.put_u32_le(viewer);
    body.put_u32_le(sid);
    body.put_u16_le(4283);
    body.put_bytes(0, 4);
    body.put_u8(1);
    body.put_u32_le(60);
    body.put_bytes(0, 4);
    body.put_u32_le(10);
    body
}

fn decode_channel(mut body: &[u8]) -> Result<Channel, NativeError> {
    match body.try_get_u16_le()? {
        200 => {}
        404 => return Err(NativeError::RoomEnded),
        code => {
            return Err(NativeError::Rejected {
                request: "room lookup",
                result: i32::from(code),
            });
        }
    }
    // The seq the caller matched.
    body.try_get_u32_le()?;
    let source = body.try_get_u32_le()?;
    let sid = body.try_get_u32_le()?;
    // The viewer's uid.
    body.try_get_u32_le()?;
    let media_ticket = Bytes::copy_from_slice(get_blob(&mut body)?);
    let timestamp = body.try_get_u32_le()?;
    body.try_get_u32_le()?;
    let audio_servers = get_servers(&mut body)?;
    let video_servers = get_servers(&mut body)?;
    Ok(Channel {
        source,
        sid,
        media_ticket,
        timestamp,
        audio_servers,
        video_servers,
        flags: suffix_flags(body),
    })
}

fn get_servers(body: &mut &[u8]) -> Result<Vec<MediaServer>, NativeError> {
    let count = body.try_get_u32_le()?;
    let mut servers = Vec::new();
    for _ in 0..count {
        let ip = Ipv4Addr::from(body.try_get_u32_le()?.to_le_bytes());
        let tcp_ports = (0..body.try_get_u32_le()?)
            .map(|_| body.try_get_u16_le())
            .collect::<Result<_, _>>()?;
        // UDP ports; media runs over TCP.
        skip_list(body, 2)?;
        servers.push(MediaServer { ip, tcp_ports });
    }
    Ok(servers)
}

/// The room flags in the suffix that may follow the server lists: 8 bytes,
/// a list of 8-byte items, then the flags. `None` when the suffix is absent
/// or shorter than that.
fn suffix_flags(mut body: &[u8]) -> Option<u32> {
    take(&mut body, 8).ok()?;
    skip_list(&mut body, 8).ok()?;
    body.try_get_u32_le().ok()
}

/// Skips a list of `item_len`-byte items behind a u32 count.
fn skip_list(body: &mut &[u8], item_len: usize) -> Result<(), NativeError> {
    let count = body.try_get_u32_le()? as usize;
    take(body, count.saturating_mul(item_len))?;
    Ok(())
}

pub(super) fn take<'a>(body: &mut &'a [u8], len: usize) -> Result<&'a [u8], NativeError> {
    if body.len() < len {
        return Err(TryGetError {
            requested: len,
            available: body.len(),
        }
        .into());
    }
    let (taken, rest) = body.split_at(len);
    *body = rest;
    Ok(taken)
}

/// A u16-length-prefixed byte string.
pub(super) fn get_blob<'a>(body: &mut &'a [u8]) -> Result<&'a [u8], NativeError> {
    let len = usize::from(body.try_get_u16_le()?);
    take(body, len)
}

pub(super) fn put_blob(out: &mut impl BufMut, data: &[u8]) {
    let data = &data[..data.len().min(usize::from(u16::MAX))];
    out.put_u16_le(data.len() as u16);
    out.put_slice(data);
}

pub(super) fn put_frame(out: &mut impl BufMut, uri: u32, body: &[u8]) {
    out.put_u32_le((HEADER_LEN + body.len()) as u32);
    out.put_u32_le(uri);
    out.put_u16_le(STATUS_OK);
    out.put_slice(body);
}

/// A positive integer as Java's `BigInteger.toByteArray` writes it:
/// big-endian, with a leading zero byte when the top bit is set.
fn java_bytes(value: &BigUint) -> Vec<u8> {
    let bytes = value.to_bytes_be();
    if bytes.first().is_some_and(|byte| byte & 0x80 != 0) {
        [&[0][..], &bytes].concat()
    } else {
        bytes
    }
}

/// An RC4 keystream.
struct Rc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// `key` must not be empty.
    fn new(key: &[u8]) -> Self {
        let mut state = [0u8; 256];
        for (slot, value) in state.iter_mut().zip(0..=u8::MAX) {
            *slot = value;
        }
        let mut j = 0u8;
        for (i, &byte) in (0..state.len()).zip(key.iter().cycle()) {
            j = j.wrapping_add(state[i]).wrapping_add(byte);
            state.swap(i, usize::from(j));
        }
        Self { state, i: 0, j: 0 }
    }

    /// Encrypts or decrypts `data` in place, continuing the keystream.
    fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.state[usize::from(self.i)]);
            self.state.swap(usize::from(self.i), usize::from(self.j));
            let index =
                self.state[usize::from(self.i)].wrapping_add(self.state[usize::from(self.j)]);
            *byte ^= self.state[usize::from(index)];
        }
    }
}

/// An ephemeral RSA-512 private key; only `n` is ever sent.
struct RsaKey {
    n: BigUint,
    d: BigUint,
}

impl RsaKey {
    fn generate() -> Self {
        let e = BigUint::from(RSA_EXPONENT);
        loop {
            let p = random_prime();
            let q = random_prime();
            let phi = (&p - 1u32) * (&q - 1u32);
            if let Some(d) = e.modinv(&phi) {
                return Self { n: p * q, d };
            }
        }
    }

    /// Decrypts a PKCS#1 v1.5 (block type 2) message.
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, NativeError> {
        let message = BigUint::from_bytes_be(ciphertext)
            .modpow(&self.d, &self.n)
            .to_bytes_be();
        // `to_bytes_be` drops leading zero bytes; the block is as long as
        // the modulus and starts with one.
        let mut block = vec![0u8; MODULUS_LEN.saturating_sub(message.len())];
        block.extend_from_slice(&message);
        unpad(&block)
    }
}

/// The message in a block `00 02 <at least 8 nonzero bytes> 00 <message>`.
fn unpad(block: &[u8]) -> Result<Vec<u8>, NativeError> {
    let invalid = || NativeError::Protocol("session key is not PKCS#1 v1.5 padded".to_owned());
    let padded = block.strip_prefix(&[0, 2]).ok_or_else(invalid)?;
    let separator = padded
        .iter()
        .position(|&byte| byte == 0)
        .ok_or_else(invalid)?;
    let message = &padded[separator + 1..];
    if separator < 8 || message.is_empty() {
        return Err(invalid());
    }
    Ok(message.to_vec())
}

/// A random 256-bit prime with its top two bits set, so that the product of
/// two is exactly 512 bits.
fn random_prime() -> BigUint {
    let mut rng = rand::rng();
    loop {
        let mut bytes = [0u8; PRIME_LEN];
        rng.fill(&mut bytes);
        bytes[0] |= 0xc0;
        bytes[PRIME_LEN - 1] |= 1;
        let candidate = BigUint::from_bytes_be(&bytes);
        if is_probable_prime(&candidate) {
            return candidate;
        }
    }
}

/// Miller–Rabin with random bases, for an odd `n` larger than every small
/// prime.
fn is_probable_prime(n: &BigUint) -> bool {
    if SMALL_PRIMES.iter().any(|&p| n % p == BigUint::ZERO) {
        return false;
    }
    let one = BigUint::from(1u32);
    let two = BigUint::from(2u32);
    let n_minus_one = n - 1u32;
    let s = n_minus_one.trailing_zeros().unwrap_or(0);
    let d = &n_minus_one >> s;
    let mut rng = rand::rng();
    'rounds: for _ in 0..MILLER_RABIN_ROUNDS {
        let mut bytes = [0u8; PRIME_LEN];
        rng.fill(&mut bytes);
        // A base in [2, n - 2].
        let base = BigUint::from_bytes_be(&bytes) % (n - 3u32) + 2u32;
        let mut x = base.modpow(&d, n);
        if x == one || x == n_minus_one {
            continue;
        }
        for _ in 1..s {
            x = x.modpow(&two, n);
            if x == n_minus_one {
                continue 'rounds;
            }
        }
        return false;
    }
    true
}

#[cfg(test)]
impl Linkd {
    /// A connection whose peer never answers, for driving the frame handling
    /// directly.
    pub(super) fn unconnected(uid: u32) -> Self {
        Self {
            io: Box::new(tokio::io::duplex(1).0),
            send: Rc4::new(b"key"),
            recv: Rc4::new(b"key"),
            input: BytesMut::new(),
            output: BytesMut::new(),
            uid,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::extractor::default::{DEFAULT_UA, default_client};
    use crate::extractor::platforms::bigo::native::ticket;

    fn hex(text: &str) -> Vec<u8> {
        let text: String = text.split_whitespace().collect();
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn rc4_matches_known_answers_across_calls() {
        for (key, plaintext, ciphertext) in [
            ("Key", "Plaintext", "bbf316e8d940af0ad3"),
            ("Secret", "Attack at dawn", "45a01f645fc35b383552544b9bf5"),
        ] {
            let mut data = plaintext.as_bytes().to_vec();
            let mut cipher = Rc4::new(key.as_bytes());
            let (head, tail) = data.split_at_mut(4);
            cipher.apply(head);
            cipher.apply(tail);
            assert_eq!(data, hex(ciphertext), "key {key:?}");
        }
    }

    #[test]
    fn decrypts_a_pkcs1_v15_session_key() {
        // A 512-bit key, and a message OpenSSL encrypted to it with
        // `pkeyutl -encrypt -pkeyopt rsa_padding_mode:pkcs1`.
        let key = RsaKey {
            n: BigUint::from_bytes_be(&hex(
                "eac4169fbfe070b96ee670dfededa72a451981ba3201804d391c1d2efde93682
                 b25281cdc4e5963097af5aca21c31ed78a38f585add3b18b61b2f175fdbf2763",
            )),
            d: BigUint::from_bytes_be(&hex(
                "8af896d7ec12a547efd133e22b0b3298801d10413356d62760763704594413827
                 a204a22e7a6b8872358809e2f13e81b6f2921f27ddac64ad1a245ba31e08481",
            )),
        };
        let ciphertext = hex(
            "5f87a7f6b7b602275c1c5a0dd4c5a9d72a050af8035aae8e2faefa4172317199
             791604aa85c5d8796d2f9203c69b60097afcb63597581829f99ade6476d76d39",
        );
        assert_eq!(key.decrypt(&ciphertext).unwrap(), b"0123456789abcdef");
    }

    #[test]
    fn a_generated_key_decrypts_what_its_public_half_encrypts() {
        let key = RsaKey::generate();
        let message = BigUint::from_bytes_be(b"session key");
        let ciphertext = message.modpow(&BigUint::from(RSA_EXPONENT), &key.n);
        assert_eq!(ciphertext.modpow(&key.d, &key.n), message);
        // A 512-bit modulus, sent with its sign byte.
        assert_eq!(java_bytes(&key.n).len(), MODULUS_LEN + 1);
    }

    #[test]
    fn frame_lengths_out_of_bounds_are_protocol_errors() {
        for len in [0, MAX_FRAME_LEN as u32 + 1] {
            let mut linkd = Linkd::unconnected(1);
            linkd.input.put_u32_le(len);
            linkd.input.put_bytes(0, HEADER_LEN);
            assert!(
                matches!(linkd.take_frame(), Err(NativeError::Protocol(_))),
                "length {len}"
            );
        }
    }

    #[test]
    fn decodes_a_captured_room_lookup() {
        // A guest's PJoinChannel2 answer for a single-host room, with made-up
        // seq, uid and room, and the media ticket replaced by the bytes
        // 0..112.
        let frame = hex(
            "a1010000c8040000c800c8000700000001e1f5050394357701e1f5057000000102030405060708090a0b0c0d0e0f1011
             12131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f4041
             42434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6fa8e3
             3ae66c73fb6a02000000a988a9090a000000c121510a69061204724b504f205746687868aa6809000000c121510a6906
             1204724b504f20574668bc4bca3f27290a0000005a21ea090206aa030b4be94eb956df6711684368090000005a21ea09
             0206aa030b4be94eb956df675b4b02000000a988a7260a0000004c24dc0cf4089d06fd4ddb51ab59d16a036b356b0900
             00004c24dc0cf4089d06fd4ddb51ab59d16a144ea988a7280a0000005923e90b0108c6050a4de850b858de69106a426a
             090000005923e90b0108aa050a4de850b858de694e4d000000000000000000000000020000800300000000000001e1f5
             050000000003943577000000000000000000000000000000000000000000000000",
        );
        let channel = decode_channel(&frame[HEADER_LEN..]).unwrap();

        assert_eq!(channel.source, 100_000_001);
        assert_eq!(channel.sid, 2_000_000_003);
        assert_eq!(channel.media_ticket, (0..112).collect::<Vec<u8>>());
        assert_eq!(channel.timestamp, 3_862_619_048);
        let ips =
            |servers: &[MediaServer]| servers.iter().map(|server| server.ip).collect::<Vec<_>>();
        assert_eq!(
            ips(&channel.audio_servers),
            [
                Ipv4Addr::new(169, 136, 169, 9),
                Ipv4Addr::new(202, 63, 39, 41)
            ]
        );
        assert_eq!(
            ips(&channel.video_servers),
            [
                Ipv4Addr::new(169, 136, 167, 38),
                Ipv4Addr::new(169, 136, 167, 40)
            ]
        );
        assert_eq!(
            channel.audio_servers[0].tcp_ports,
            [
                8641, 2641, 1641, 1042, 19314, 20304, 22304, 26694, 26744, 26794
            ]
        );
        assert_eq!(channel.flags, Some(0x8000_0002));
        assert!(!channel.multi_guest());
    }

    #[tokio::test]
    #[ignore = "signs in to BIGO's Linkd as a guest"]
    async fn a_guest_looks_up_a_live_room() {
        let route = Route {
            client: default_client(),
            proxy: None,
        };
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
        let rooms = list["data"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|room| room["room_id"].as_u64())
            .take(3);

        // A listed room may have ended since.
        for room_id in rooms {
            let ticket = ticket::guest(&route).await.unwrap();
            let linkd = Linkd::connect(&route, &ticket).await.unwrap();
            match linkd.join_channel(room_id).await {
                Err(NativeError::RoomEnded) => continue,
                result => {
                    let channel = result.unwrap();
                    assert_eq!(channel.sid, room_id as u32);
                    assert!(!channel.media_ticket.is_empty());
                    assert!(!channel.audio_servers.is_empty());
                    assert!(!channel.video_servers.is_empty());
                    return;
                }
            }
        }
        panic!("every listed room had ended");
    }
}
