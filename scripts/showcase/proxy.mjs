// Reverse proxy between the frontend and a local backend. Every HTTP request
// and WebSocket upgrade passes through unchanged, except the download-progress
// socket: it answers with a fixed snapshot of fictional in-progress
// recordings, so the dashboard shows live progress without recording a real
// stream.
import { createHash } from 'node:crypto';
import http from 'node:http';
import net from 'node:net';

// --- protobuf wire encoding (only what download_progress.proto needs) ---

function varint(n) {
  let v = BigInt(n);
  const out = [];
  do {
    let byte = Number(v & 0x7fn);
    v >>= 7n;
    if (v) byte |= 0x80;
    out.push(byte);
  } while (v);
  return Buffer.from(out);
}
const tag = (field, wire) => varint((field << 3) | wire);
const pbVarint = (field, n) =>
  n ? Buffer.concat([tag(field, 0), varint(n)]) : Buffer.alloc(0);
const pbBytes = (field, buf) =>
  Buffer.concat([tag(field, 2), varint(buf.length), buf]);
const pbString = (field, s) => (s ? pbBytes(field, Buffer.from(s)) : Buffer.alloc(0));
const pbDouble = (field, x) => {
  const b = Buffer.alloc(8);
  b.writeDoubleLE(x);
  return Buffer.concat([tag(field, 1), b]);
};
const pbBool = (field, v) => pbVarint(field, v ? 1 : 0);

const EVENT_TYPE_SNAPSHOT = 1;

/**
 * `recordings` items: { downloadId, streamerId, sessionId, engine, cdnHost,
 * startedAtMs, bytes, speed, segments, mediaSecs }.
 */
export function encodeSnapshot(recordings, now = Date.now()) {
  const downloads = recordings.map((r) => {
    const elapsed = (now - r.startedAtMs) / 1000;
    const meta = Buffer.concat([
      pbString(1, r.downloadId),
      pbString(2, r.streamerId),
      pbString(3, r.sessionId),
      pbString(4, r.engine),
      pbVarint(5, r.startedAtMs),
      pbString(6, r.cdnHost),
      pbString(7, `https://${r.cdnHost}/live/stream.flv`),
      pbVarint(8, now),
      pbBytes(9, pbBool(1, true)),
    ]);
    const metrics = Buffer.concat([
      pbString(1, r.downloadId),
      pbString(2, 'Downloading'),
      pbVarint(3, r.bytes),
      pbDouble(4, elapsed),
      pbVarint(5, r.speed),
      pbVarint(6, r.segments),
      pbDouble(7, r.mediaSecs ?? elapsed),
      pbDouble(8, 1.0),
    ]);
    return pbBytes(1, Buffer.concat([pbBytes(1, meta), pbBytes(2, metrics)]));
  });
  const snapshot = Buffer.concat(downloads);
  return Buffer.concat([pbVarint(1, EVENT_TYPE_SNAPSHOT), pbBytes(2, snapshot)]);
}

// --- minimal server-side WebSocket ---

function wsFrame(opcode, payload) {
  const len = payload.length;
  let header;
  if (len < 126) header = Buffer.from([0x80 | opcode, len]);
  else if (len < 65536) {
    header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 126;
    header.writeUInt16BE(len, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(len), 2);
  }
  return Buffer.concat([header, payload]);
}

function acceptWebSocket(req, socket) {
  const key = req.headers['sec-websocket-key'];
  const accept = createHash('sha1')
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest('base64');
  socket.write(
    'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n' +
      `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  // Client frames (subscribe requests, pings) need no answer for a static
  // snapshot; a close frame ends the connection.
  socket.on('data', (buf) => {
    if ((buf[0] & 0x0f) === 0x8) socket.end(wsFrame(0x8, Buffer.alloc(0)));
  });
  socket.on('error', () => {});
}

// --- proxy ---

// The backend resets every seeded LIVE streamer to NOT_LIVE at startup,
// because no recording is running for it, and serves state from memory. The
// proxy therefore reports the fictional live streamers as LIVE itself.
const STREAMER_PATH = /^\/api\/streamers(?:\/([^/?]+))?(?:\?|$)/;

function patchStreamer(s, liveIds) {
  // Seeded streamers carry a far-future backoff so the backend never checks
  // their made-up URLs; hide it so the cards show no recovery countdown.
  const out = { ...s, disabled_until: null };
  if (liveIds.has(s.id)) out.state = 'LIVE';
  return out;
}

function rewriteStreamers(json, liveIds, liveOnly) {
  if (Array.isArray(json.items)) {
    let items = json.items.map((s) => patchStreamer(s, liveIds));
    if (liveOnly) items = items.filter((s) => s.state === 'LIVE');
    return { ...json, items, total: liveOnly ? items.length : json.total };
  }
  return json.id ? patchStreamer(json, liveIds) : json;
}

/**
 * `pathAlias` = [from, to] replaces the temporary work directory in every
 * JSON response, so paths read like a real install.
 */
export function startProxy({ port, backendPort, recordings, liveIds, pathAlias }) {
  const server = http.createServer((req, res) => {
    const match = req.method === 'GET' && STREAMER_PATH.exec(req.url ?? '');
    let path = req.url;
    let liveOnly = false;
    if (match && !match[1]) {
      const url = new URL(req.url, 'http://x');
      if (url.searchParams.get('state') === 'LIVE') {
        // Filter after patching: the backend's LIVE filter sees NOT_LIVE.
        liveOnly = true;
        url.searchParams.delete('state');
        url.searchParams.set('limit', '500');
        url.searchParams.delete('offset');
      }
      path = url.pathname + url.search;
    }
    const headers = { ...req.headers, host: `127.0.0.1:${backendPort}` };
    delete headers['accept-encoding'];
    const upstream = http.request(
      { host: '127.0.0.1', port: backendPort, method: req.method, path, headers },
      (up) => {
        const isJson = String(up.headers['content-type']).includes('json');
        if (!isJson) {
          res.writeHead(up.statusCode ?? 502, up.headers);
          up.pipe(res);
          return;
        }
        const chunks = [];
        up.on('data', (c) => chunks.push(c));
        up.on('end', () => {
          let body = Buffer.concat(chunks).toString();
          if (pathAlias) body = body.replaceAll(pathAlias[0], pathAlias[1]);
          if (match && up.statusCode === 200) {
            body = JSON.stringify(rewriteStreamers(JSON.parse(body), liveIds, liveOnly));
          }
          const outHeaders = { ...up.headers };
          delete outHeaders['content-length'];
          delete outHeaders['transfer-encoding'];
          res.writeHead(up.statusCode ?? 502, outHeaders);
          res.end(body);
        });
      },
    );
    upstream.on('error', () => {
      if (!res.headersSent) res.writeHead(502);
      res.end();
    });
    req.pipe(upstream);
  });

  server.on('upgrade', (req, socket, head) => {
    if (req.url?.startsWith('/api/downloads/ws')) {
      acceptWebSocket(req, socket);
      socket.write(wsFrame(0x2, encodeSnapshot(recordings())));
      return;
    }
    const upstream = net.connect(backendPort, '127.0.0.1', () => {
      const headers = Object.entries(req.headers)
        .map(([k, v]) => `${k}: ${v}`)
        .join('\r\n');
      upstream.write(`${req.method} ${req.url} HTTP/1.1\r\n${headers}\r\n\r\n`);
      upstream.write(head);
      upstream.pipe(socket);
      socket.pipe(upstream);
    });
    upstream.on('error', () => socket.destroy());
    socket.on('error', () => upstream.destroy());
  });

  return new Promise((resolve) =>
    server.listen(port, '127.0.0.1', () => resolve(server)),
  );
}
