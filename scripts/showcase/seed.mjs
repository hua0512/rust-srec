// Fictional demo data for the showcase captures. Every name, title, chat
// line and avatar here is made up; URLs point at channels that do not exist,
// and each streamer carries a far-future backoff so the backend never checks
// them.
import { execFile } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { promisify } from 'node:util';

const run = promisify(execFile);
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

// Deterministic PRNG so every run produces the same numbers.
function rng(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function uuid(random) {
  const h = Array.from({ length: 32 }, () => Math.floor(random() * 16).toString(16));
  // Version 4, variant 8 or 9: a valid UUID whose fourth group starts with a
  // digit, so the breadcrumb's CSS `capitalize` leaves it lower-case.
  h[12] = '4';
  h[16] = parseInt(h[16], 16) & 1 ? '9' : '8';
  return `${h.slice(0, 8).join('')}-${h.slice(8, 12).join('')}-${h.slice(12, 16).join('')}-${h.slice(16, 20).join('')}-${h.slice(20).join('')}`;
}

export const STREAMERS = [
  { id: 'aurora', name: 'Aurora', platform: 'twitch', url: 'https://www.twitch.tv/aurora_showcase', live: true, priority: 'HIGH', colors: ['#f97316', '#db2777'],
    titles: ['Speedrun practice — any% route', 'Late night chill & chat', 'Community game night'] },
  { id: 'nightjar', name: '夜航星', platform: 'huya', url: 'https://www.huya.com/990001', live: true, priority: 'HIGH', colors: ['#6366f1', '#06b6d4'],
    titles: ['深夜电台 · 点歌环节', '新赛季冲分', '周末畅聊'] },
  { id: 'mochi', name: 'Mochi', platform: 'bilibili', url: 'https://live.bilibili.com/990002', live: true, priority: 'NORMAL', colors: ['#ec4899', '#f59e0b'],
    titles: ['手绘插画直播', 'Drawing stream: character sheets', '杂谈 + 抽奖'] },
  { id: 'galaxy', name: '星河', platform: 'douyin', url: 'https://live.douyin.com/990003', priority: 'NORMAL', colors: ['#8b5cf6', '#22d3ee'],
    titles: ['户外徒步 · 云海日出', '城市夜景漫步', '露营 vlog'] },
  { id: 'kestrel', name: 'Kestrel', platform: 'douyu', url: 'https://www.douyu.com/990004', priority: 'NORMAL', colors: ['#10b981', '#3b82f6'],
    titles: ['Ranked grind to Diamond', 'Viewer games', 'Patch notes review'] },
  { id: 'pixelfox', name: 'Pixelfox', platform: 'soop', url: 'https://ch.sooplive.co.kr/pixelfox990', priority: 'NORMAL', colors: ['#f43f5e', '#fb923c'],
    titles: ['Retro platformer marathon', 'Pixel art timelapse', 'Indie game showcase'] },
  { id: 'tidewater', name: 'Tidewater', platform: 'twitcasting', url: 'https://twitcasting.tv/tidewater_990', priority: 'LOW', colors: ['#0ea5e9', '#14b8a6'],
    titles: ['Acoustic covers', 'Morning coffee talk', 'Songwriting session'] },
  { id: 'orbit', name: 'Orbit', platform: 'picarto', url: 'https://picarto.tv/orbit990', priority: 'NORMAL', colors: ['#a855f7', '#ec4899'],
    titles: ['3D modelling: spaceship', 'Concept art', 'Sketchbook Sunday'] },
  { id: 'juniper', name: 'Juniper', platform: 'tiktok', url: 'https://www.tiktok.com/@juniper990/live', priority: 'NORMAL', colors: ['#84cc16', '#22c55e'],
    titles: ['Cooking: hand-pulled noodles', 'Q&A while baking', 'Market haul'] },
  { id: 'lantern', name: '灯笼', platform: 'acfun', url: 'https://live.acfun.cn/live/990005', priority: 'LOW', colors: ['#eab308', '#ef4444'],
    titles: ['单机游戏通关', '恐怖游戏挑战', '剧情向游戏'] },
  { id: 'vesper', name: 'Vesper', platform: 'bigo', url: 'https://www.bigo.tv/990006', priority: 'LOW', colors: ['#64748b', '#6366f1'],
    titles: ['Piano requests', 'Lo-fi beats making', 'Late night jam'] },
];

const CHAT_USERS = [
  'moonlit_otter', 'pixel_pilot', '小鱼干', 'cosmic_toast', 'driftwood', '风铃草',
  'nebula_cat', 'quietstorm', 'tea_and_code', '晚风', 'lumen_fan', 'paper_crane',
];
// Streamers on Chinese platforms get Chinese chat words and gift names.
const CHINESE_PLATFORMS = new Set(['huya', 'bilibili', 'douyin', 'douyu', 'acfun']);
const CHAT_WORDS = [
  ['GG', 'hype', 'clip it', 'lol', 'POG', 'nice', 'gl hf', 'wow', 'first', 'encore'],
  ['哈哈哈', '666', '好听', '来了', '晚上好', '加油', '好看', '冲', '下次见', '牛'],
];

function avatar(name, [c0, c1]) {
  const initial = [...name][0].toUpperCase();
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${c0}"/>` +
    `<stop offset="1" stop-color="${c1}"/></linearGradient></defs>` +
    `<rect width="128" height="128" fill="url(#g)"/>` +
    `<circle cx="104" cy="22" r="40" fill="#fff" fill-opacity=".14"/>` +
    `<text x="64" y="64" dy=".35em" text-anchor="middle" font-family="Inter,'Noto Sans CJK SC',sans-serif" ` +
    `font-size="60" font-weight="700" fill="#fff">${initial}</text></svg>`;
  return `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`;
}

async function thumbnail(path, [c0, c1], seed) {
  // A soft two-colour gradient with drifting light blobs: reads as a video
  // frame at card size without depicting anyone.
  const hex = (c) => `0x${c.slice(1)}`;
  const filter =
    `gradients=s=640x360:c0=${hex(c0)}:c1=${hex(c1)}:x0=0:y0=0:x1=640:y1=360:nb_colors=2:seed=${seed}:duration=1,` +
    `format=rgb24`;
  await run('ffmpeg', ['-y', '-v', 'error', '-f', 'lavfi', '-i', filter, '-frames:v', '1', '-q:v', '3', path]);
}

/** Seeds an already-migrated database. Returns what the proxy needs. */
export async function seedDatabase({ dbPath, outputDir, now = Date.now() }) {
  const db = new DatabaseSync(dbPath);
  const random = rng(20261114);
  const parked = now + 365 * DAY;
  const thumbsDir = join(outputDir, 'thumbnails');
  await mkdir(thumbsDir, { recursive: true });

  const insertStreamer = db.prepare(
    `INSERT INTO streamers (id, name, url, platform_config_id, state, priority,
       last_live_time, disabled_until, avatar, created_at, updated_at)
     VALUES (?, ?, ?, ?, 'NOT_LIVE', ?, ?, ?, ?, ?, ?)`,
  );
  const insertSession = db.prepare(
    `INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time,
       titles, danmu_statistics_id, total_size_bytes, session_complete_dispatched)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)`,
  );
  const insertOutput = db.prepare(
    `INSERT INTO media_outputs (id, session_id, file_path, file_type, size_bytes, created_at)
     VALUES (?, ?, ?, ?, ?, ?)`,
  );
  const insertSegment = db.prepare(
    `INSERT INTO session_segments (id, session_id, segment_index, file_path, duration_secs,
       size_bytes, split_reason_code, created_at, completed_at, persisted_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );
  const insertDanmu = db.prepare(
    `INSERT INTO danmu_statistics (id, session_id, total_danmus, unique_talkers, chat_count,
       gift_count, duration_secs, start_time, end_time, rate_bucket_secs,
       danmu_rate_timeseries, top_talkers, top_gifters, top_gifts, word_frequency)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );
  const insertCheck = db.prepare(
    `INSERT INTO streamer_check_history (streamer_id, checked_at, duration_ms, outcome,
       streams_extracted, stream_selected, title, category, viewer_count)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );

  const recordings = [];
  const sessions = [];
  const thumbJobs = [];
  // Sessions and their chat statistics reference each other.
  db.exec('PRAGMA defer_foreign_keys = ON; BEGIN');
  STREAMERS.forEach((s, si) => {
    const lastLive = s.live ? now : now - (2 + si * 5) * HOUR;
    insertStreamer.run(
      s.id, s.name, s.url, `platform-${s.platform}`, s.priority, lastLive, parked,
      avatar(s.name, s.colors), now - 40 * DAY, now - DAY,
    );

    // Past sessions over the last week, newest first.
    let end = s.live ? now - 20 * HOUR : lastLive;
    const count = 3 + (si % 3);
    for (let k = 0; k < count; k++) {
      const duration = (1.5 + random() * 4) * HOUR;
      const start = end - duration;
      const id = uuid(random);
      sessions.push({ id, streamerId: s.id, index: k });
      const title = s.titles[k % s.titles.length];
      const bytesPerSec = 400_000 + random() * 600_000;
      const size = Math.round((duration / 1000) * bytesPerSec);
      const danmuId = `danmu-${id}`;
      insertSession.run(
        id, s.id, s.name, Math.round(start), Math.round(end),
        JSON.stringify([{ ts: Math.round(start), title }]), danmuId, size,
      );

      const parts = 1 + Math.floor(duration / (2 * HOUR));
      for (let p = 0; p < parts; p++) {
        const file = join(outputDir, s.id, `${s.id}-${new Date(start).toISOString().slice(0, 10)}-part${p + 1}.mp4`);
        const partSize = Math.round(size / parts);
        const partStart = Math.round(start + (p * duration) / parts);
        const partEnd = Math.round(start + ((p + 1) * duration) / parts);
        insertOutput.run(`out-${id}-v${p}`, id, file, 'VIDEO', partSize, partEnd);
        insertSegment.run(
          `seg-${id}-${p}`, id, p, file, (partEnd - partStart) / 1000, partSize,
          p + 1 < parts ? 'size_limit' : 'stream_ended', partStart, partEnd, partEnd,
        );
      }
      insertOutput.run(`out-${id}-x`, id, join(outputDir, s.id, `${id}.xml`), 'DANMU_XML', Math.round(size / 900), Math.round(end));
      const thumb = join(thumbsDir, `${id}.jpg`);
      thumbJobs.push(thumbnail(thumb, k % 2 ? [...s.colors].reverse() : s.colors, si * 10 + k));
      insertOutput.run(`out-${id}-t`, id, thumb, 'THUMBNAIL', 48_000, Math.round(end));

      // Chat statistics: an evening-shaped curve with a mid-stream peak.
      const bucket = 60;
      const buckets = Math.floor(duration / 1000 / bucket);
      const chinese = CHINESE_PLATFORMS.has(s.platform);
      const words = CHAT_WORDS[chinese ? 1 : 0];
      const rate = [];
      let total = 0;
      for (let b = 0; b < buckets; b++) {
        const x = b / buckets;
        const shape = Math.sin(Math.PI * x) ** 0.6 + 0.6 * Math.exp(-((x - 0.62) ** 2) / 0.004);
        const c = Math.round((20 + 70 * shape) * (0.75 + random() * 0.5));
        total += c;
        rate.push({ ts: Math.round(start + b * bucket * 1000), count: c });
      }
      const talkers = CHAT_USERS.map((u, i) => ({
        user_id: `u${i}`, username: u, message_count: Math.round(total * (0.05 / (i + 1)) + random() * 30), error: 0,
      }));
      const gifters = CHAT_USERS.slice(3, 9).map((u, i) => ({
        user_id: `g${i}`, username: u, message_count: Math.round(60 / (i + 1) + random() * 10), error: 0,
      }));
      const freq = words.map((w, i) => ({ word: w, count: Math.round(total * (0.08 / (i + 1)) + random() * 40), error: 0 }));
      const gifts = [
        { name: chinese ? '火箭' : 'Rocket', count: 12 + Math.round(random() * 20) },
        { name: chinese ? '星星' : 'Star', count: 80 + Math.round(random() * 120) },
        { name: chinese ? '小心心' : 'Heart', count: 300 + Math.round(random() * 400) },
      ];
      insertDanmu.run(
        danmuId, id, total, 600 + Math.round(random() * 900), Math.round(total * 0.93),
        Math.round(total * 0.07), Math.round(duration / 1000), Math.round(start), Math.round(end), bucket,
        JSON.stringify(rate), JSON.stringify(talkers), JSON.stringify(gifters), JSON.stringify(gifts), JSON.stringify(freq),
      );
      end = start - (8 + random() * 30) * HOUR;
    }

    // Recent check history: a run of offline checks, live ones for live
    // streamers and around their last session.
    for (let c = 0; c < 48; c++) {
      const at = now - (48 - c) * 2 * MIN;
      const isLive = s.live && c > 30;
      insertCheck.run(
        s.id, at, 180 + Math.round(random() * 500), isLive ? 'live' : 'offline',
        isLive ? 4 : 0,
        isLive ? JSON.stringify({ quality: '1080p60', stream_format: 'flv', media_format: 'flv', bitrate: 6000, codec: 'avc', fps: 60 }) : null,
        isLive ? s.titles[0] : null, isLive ? 'Just Chatting' : null,
        isLive ? 800 + Math.round(random() * 9000) : null,
      );
    }

    if (s.live) {
      const startedAt = now - (0.6 + random() * 2.5) * HOUR;
      const speed = 450_000 + Math.round(random() * 700_000);
      recordings.push({
        downloadId: `dl-${s.id}`,
        streamerId: s.id,
        sessionId: uuid(random),
        engine: si % 2 ? 'ffmpeg' : 'mesio',
        cdnHost: `edge-${si + 1}.cdn.example.com`,
        startedAtMs: Math.round(startedAt),
        bytes: Math.round(((now - startedAt) / 1000) * speed),
        speed,
        segments: 1 + Math.floor((now - startedAt) / HOUR),
      });
    }
  });
  db.exec('COMMIT');
  db.close();
  await Promise.all(thumbJobs);

  return {
    liveIds: new Set(STREAMERS.filter((s) => s.live).map((s) => s.id)),
    recordings,
    sessions,
  };
}

async function clip(path, { seconds, size }) {
  await run('ffmpeg', [
    '-y', '-v', 'error',
    '-f', 'lavfi', '-i', `testsrc2=s=${size}:r=30:d=${seconds}`,
    '-f', 'lavfi', '-i', `sine=f=440:d=${seconds}`,
    '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac', '-shortest', path,
  ]);
}

async function api(base, method, path, body) {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { 'content-type': 'application/json', connection: 'close' },
    body: body && JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
  return res.json();
}

const presetStep = (id, name, dependsOn = []) => ({
  id,
  step: { type: 'preset', name },
  depends_on: dependsOn,
});

/**
 * Runs real pipeline jobs on generated test clips: quick remux/thumbnail
 * workflows that complete, uploads that fail because no rclone remote is
 * configured, and a slow encode that stays in progress with one more queued
 * behind it. Resolves once the quick jobs have settled.
 */
export async function createJobs({ apiBase, outputDir, sessions }) {
  const dir = join(outputDir, 'jobs');
  await mkdir(dir, { recursive: true });
  // One CPU slot: the slow encode runs, the next one waits in the queue.
  await api(apiBase, 'PATCH', '/config/global', { max_concurrent_cpu_jobs: 1 });

  const quick = sessions.filter((s) => s.index === 0).slice(0, 7);
  const clips = await Promise.all(
    quick.map(async (s, i) => {
      const path = join(dir, `${s.streamerId}-${i}.mp4`);
      await clip(path, { seconds: 6, size: '640x360' });
      return path;
    }),
  );
  for (const [i, s] of quick.entries()) {
    const steps = [presetStep('remux', 'remux'), presetStep('thumbnail', 'thumbnail', ['remux'])];
    if (i % 3 === 2) steps.push(presetStep('upload', 'upload', ['remux', 'thumbnail']));
    await api(apiBase, 'POST', '/pipeline/create', {
      session_id: s.id,
      streamer_id: s.streamerId,
      input_paths: [clips[i]],
      dag: { name: i % 3 === 2 ? 'Archive to Cloud' : 'Standard', steps },
    });
  }

  // Wait for the quick jobs before the slow encode takes the CPU slot.
  for (let t = 0; t < 240; t++) {
    const stats = await api(apiBase, 'GET', '/pipeline/stats');
    if (stats.pending_count === 0 && stats.processing_count === 0) break;
    await new Promise((r) => setTimeout(r, 500));
  }

  const slow = sessions.filter((s) => s.index === 1).slice(0, 2);
  for (const [i, s] of slow.entries()) {
    const path = join(dir, `${s.streamerId}-long.mp4`);
    await clip(path, { seconds: 240, size: '1920x1080' });
    await api(apiBase, 'POST', '/pipeline/create', {
      session_id: s.id,
      streamer_id: s.streamerId,
      input_paths: [path],
      dag: {
        name: 'Space Saver',
        steps: [presetStep('compress', 'compress_hevc_max')],
      },
    });
    if (i === 0) await new Promise((r) => setTimeout(r, 4000));
  }
}
