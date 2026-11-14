// Generated pictures and media for the captures: a drawn landscape for every
// session thumbnail, and for the featured sessions a short video of one with
// a Bilibili-format chat XML beside it, so the player shows chat over the
// video. Scenes are SVG rendered by Chromium. Nothing here comes from a real
// stream.
import { execFile } from 'node:child_process';
import { copyFile, mkdir, rm, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import { promisify } from 'node:util';

import { CHAT_USERS, STREAMERS, rng } from './seed.mjs';

const run = promisify(execFile);

const WIDTH = 1280;
const HEIGHT = 720;
const SECONDS = 30;

// The session shown in the chat statistics and player shots, per locale: a
// stream whose chat is in that language.
export const FEATURED = { en: 'aurora', 'zh-CN': 'nightjar' };

const CHAT = {
  aurora: [
    'GG', 'hype!!', 'this view is so calm', 'first time here, love the vibe',
    'night sky + lofi = perfect', 'clip it', 'LOL', 'hi from Berlin',
    'what song is this?', 'POG', 'gl hf', 'so cozy', 'encore!', 'that moon tho',
    'chat is so wholesome tonight', 'nice', 'wow', 'good evening everyone',
    '10/10 stream', "can't sleep, perfect timing",
  ],
  nightjar: [
    '晚上好', '哈哈哈哈', '666', '好美的夜景', '这首歌叫什么？', '前排打卡',
    '氛围感拉满', '主播声音好好听', '下次见', '加油', '冲冲冲', '好看',
    '太治愈了', '第一次来，关注了', '来了来了', '月亮好圆', '失眠的人来报到',
    '弹幕护体', '牛', '晚安',
  ],
};

// Bilibili danmu colours, as 0xRRGGBB.
const COLOURS = [0xfe0302, 0xffff00, 0x00cd00, 0x89d5ff, 0xcc0273];

const escapeXml = (text) =>
  text.replace(/[<>&"']/g, (c) => `&#${c.charCodeAt(0)};`);

// A ridge line across the frame as an SVG path closed along the bottom.
function ridge(random, { base, amplitude, step, bottom = HEIGHT }) {
  let y = base;
  const points = [];
  for (let x = -step; x <= WIDTH + step; x += step) {
    y += (random() - 0.5) * amplitude;
    y = Math.min(Math.max(y, base - amplitude * 1.6), base + amplitude * 0.8);
    points.push(`${x},${y.toFixed(1)}`);
  }
  return `M${points.join(' L')} L${WIDTH + step},${bottom} L${-step},${bottom} Z`;
}

function stars(random, count, maxY) {
  return Array.from({ length: count }, () => {
    const x = random() * WIDTH;
    const y = random() * maxY;
    const r = 0.6 + random() * 1.4;
    return `<circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="${r.toFixed(2)}" fill="#fff" fill-opacity="${(0.3 + random() * 0.6).toFixed(2)}"/>`;
  }).join('');
}

function pines(random, { count, baseY }) {
  return Array.from({ length: count }, (_, i) => {
    const x = (i / count) * WIDTH + random() * 30;
    const h = 26 + random() * 46;
    const y = baseY - random() * 30;
    return `<path d="M${x},${y - h} L${x + h * 0.32},${y} L${x - h * 0.32},${y} Z" fill="#06040f"/>`;
  }).join('');
}

const sky = (stops) =>
  `<linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">${stops
    .map(([o, c]) => `<stop offset="${o}" stop-color="${c}"/>`)
    .join('')}</linearGradient>`;

const glow = (colour, opacity) =>
  `<radialGradient id="glow"><stop offset="0" stop-color="${colour}" stop-opacity="${opacity}"/><stop offset="1" stop-color="${colour}" stop-opacity="0"/></radialGradient>`;

// Each kind returns SVG defs and body for a 1280×720 landscape in the
// streamer's colours.
const SCENES = {
  mountains(random, [c0, c1]) {
    return {
      defs: sky([[0, '#070a1c'], [0.45, '#1b1640'], [0.78, c1], [1, c0]]) + glow('#fff4dc', 0.55),
      body: `<rect width="${WIDTH}" height="${HEIGHT}" fill="url(#sky)"/>
        ${stars(random, 140, HEIGHT * 0.55)}
        <circle cx="960" cy="170" r="170" fill="url(#glow)"/>
        <circle cx="960" cy="170" r="48" fill="#fff6e3"/>
        <path d="${ridge(random, { base: 470, amplitude: 60, step: 64 })}" fill="#2a2150" fill-opacity="0.9"/>
        <path d="${ridge(random, { base: 540, amplitude: 46, step: 48 })}" fill="#17112f"/>
        <path d="${ridge(random, { base: 620, amplitude: 28, step: 40 })}" fill="#0b0819"/>
        ${pines(random, { count: 34, baseY: HEIGHT - 70 })}`,
    };
  },
  lake(random, [c0, c1]) {
    const far = ridge(random, { base: 380, amplitude: 70, step: 56, bottom: 430 });
    const near = ridge(random, { base: 410, amplitude: 36, step: 40, bottom: 430 });
    return {
      defs: sky([[0, '#060a1a'], [0.4, '#16183d'], [0.6, c1]]) + glow('#e8f0ff', 0.5) +
        `<linearGradient id="water" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${c1}" stop-opacity="0.55"/><stop offset="1" stop-color="#05060f"/></linearGradient>`,
      body: `<rect width="${WIDTH}" height="${HEIGHT}" fill="url(#sky)"/>
        ${stars(random, 110, 340)}
        <circle cx="330" cy="150" r="150" fill="url(#glow)"/>
        <circle cx="330" cy="150" r="40" fill="#f3f6ff"/>
        <path d="${far}" fill="#25204a"/>
        <path d="${near}" fill="#120f29"/>
        <rect y="430" width="${WIDTH}" height="${HEIGHT - 430}" fill="url(#water)"/>
        <g transform="translate(0 860) scale(1 -1)" opacity="0.35"><path d="${far}" fill="#25204a"/><path d="${near}" fill="#120f29"/></g>
        ${Array.from({ length: 9 }, (_, i) => `<rect x="${300 - i * 4 + random() * 20}" y="${450 + i * 26}" width="${60 + i * 10}" height="3" rx="1.5" fill="#f3f6ff" fill-opacity="${(0.5 - i * 0.045).toFixed(2)}"/>`).join('')}
        <rect y="428" width="${WIDTH}" height="3" fill="${c0}" fill-opacity="0.35"/>`,
    };
  },
  city(random, [c0, c1]) {
    const buildings = [];
    for (let x = -10; x < WIDTH; ) {
      const w = 50 + random() * 90;
      const h = 140 + random() * 300;
      const top = HEIGHT - h;
      buildings.push(`<rect x="${x.toFixed(1)}" y="${top.toFixed(1)}" width="${w.toFixed(1)}" height="${h.toFixed(1)}" fill="#0a0918"/>`);
      for (let wy = top + 14; wy < HEIGHT - 16; wy += 22) {
        for (let wx = x + 10; wx < x + w - 14; wx += 18) {
          if (random() < 0.42) {
            buildings.push(`<rect x="${wx.toFixed(1)}" y="${wy.toFixed(1)}" width="8" height="11" rx="1" fill="${random() < 0.8 ? '#ffd27a' : c0}" fill-opacity="${(0.45 + random() * 0.5).toFixed(2)}"/>`);
          }
        }
      }
      x += w + 6 + random() * 10;
    }
    return {
      defs: sky([[0, '#0b0b24'], [0.5, '#2a1c4f'], [0.8, c1], [1, c0]]),
      body: `<rect width="${WIDTH}" height="${HEIGHT}" fill="url(#sky)"/>
        ${stars(random, 60, 260)}
        <path d="${ridge(random, { base: 420, amplitude: 40, step: 60 })}" fill="#1c1538" fill-opacity="0.8"/>
        ${buildings.join('')}`,
    };
  },
  sunset(random, [c0, c1]) {
    return {
      defs: sky([[0, '#2b1a4f'], [0.35, c1], [0.7, c0], [1, '#ffd28a']]) + glow('#fff1c1', 0.75),
      body: `<rect width="${WIDTH}" height="${HEIGHT}" fill="url(#sky)"/>
        <circle cx="640" cy="520" r="260" fill="url(#glow)"/>
        <circle cx="640" cy="520" r="96" fill="#fff3d1"/>
        <path d="${ridge(random, { base: 520, amplitude: 50, step: 70 })}" fill="#7a2d5c" fill-opacity="0.75"/>
        <path d="${ridge(random, { base: 580, amplitude: 36, step: 54 })}" fill="#4a1d47"/>
        <path d="${ridge(random, { base: 640, amplitude: 22, step: 44 })}" fill="#24102b"/>
        ${pines(random, { count: 28, baseY: HEIGHT - 50 })}`,
    };
  },
  aurora(random, [c0, c1]) {
    const band = (y, colour, opacity) =>
      `<path d="M-50,${y} C 250,${y - 120} 500,${y + 80} 760,${y - 40} S 1150,${y - 140} 1330,${y - 30}" stroke="${colour}" stroke-width="90" fill="none" stroke-opacity="${opacity}" filter="url(#soft)"/>`;
    return {
      defs: sky([[0, '#030712'], [0.6, '#0b1a2e'], [1, '#13294a']]) +
        `<filter id="soft" x="-20%" y="-50%" width="140%" height="200%"><feGaussianBlur stdDeviation="28"/></filter>`,
      body: `<rect width="${WIDTH}" height="${HEIGHT}" fill="url(#sky)"/>
        ${stars(random, 160, HEIGHT * 0.6)}
        ${band(230, '#34d399', 0.55)}
        ${band(300, c0, 0.35)}
        ${band(190, c1, 0.3)}
        <path d="${ridge(random, { base: 500, amplitude: 70, step: 60 })}" fill="#d7e3f4" fill-opacity="0.9"/>
        <path d="${ridge(random, { base: 540, amplitude: 50, step: 50 })}" fill="#22324d"/>
        <path d="${ridge(random, { base: 620, amplitude: 26, step: 40 })}" fill="#0b1322"/>
        ${pines(random, { count: 36, baseY: HEIGHT - 60 })}`,
    };
  },
};

const KINDS = Object.keys(SCENES);

// The name and title a streaming overlay would show in a corner.
function lowerThird(streamer, title) {
  const [c0, c1] = streamer.colors;
  const initial = [...streamer.name][0].toUpperCase();
  return `<defs><linearGradient id="avatar" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${c0}"/><stop offset="1" stop-color="${c1}"/></linearGradient></defs>
  <g transform="translate(40 ${HEIGHT - 112})">
    <rect width="${Math.max(360, 150 + [...title].length * 17)}" height="72" rx="16" fill="#000" fill-opacity="0.45"/>
    <circle cx="40" cy="36" r="22" fill="url(#avatar)"/>
    <text x="40" y="36" dy=".35em" text-anchor="middle" font-size="22" font-weight="700" fill="#fff">${escapeXml(initial)}</text>
    <text x="76" y="30" font-size="20" font-weight="700" fill="#fff">${escapeXml(streamer.name)}</text>
    <text x="76" y="54" font-size="16" fill="#fff" fill-opacity="0.8">${escapeXml(title)}</text>
  </g>`;
}

function scene(streamer, session, { overlay = false } = {}) {
  const si = STREAMERS.indexOf(streamer);
  const kind = KINDS[(si + session.index) % KINDS.length];
  const random = rng(si * 7919 + session.index * 104729 + 1);
  const { defs, body } = SCENES[kind](random, streamer.colors);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${WIDTH}" height="${HEIGHT}" viewBox="0 0 ${WIDTH} ${HEIGHT}">
  <defs>${defs}</defs>
  ${body}
  ${overlay ? lowerThird(streamer, session.title) : ''}
</svg>`;
}

async function renderScene(page, svg, format) {
  const html = `<!doctype html><meta charset="utf-8"><style>
    body{margin:0;font-family:Inter,'Noto Sans CJK SC','PingFang SC',sans-serif}</style>${svg}`;
  await page.goto(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`);
  const { data } = await page.send('Page.captureScreenshot', {
    format,
    ...(format === 'jpeg' ? { quality: 85 } : {}),
  });
  return Buffer.from(data, 'base64');
}

function danmuXml(streamer) {
  const random = rng(streamer.id.length * 104729);
  const lines = CHAT[streamer.id];
  const rows = [];
  for (let i = 0, t = 0.2; t < SECONDS; i++) {
    t += 0.3 + random() * 0.5;
    const text = lines[Math.floor(random() * lines.length)];
    const colour = random() < 0.18 ? COLOURS[Math.floor(random() * COLOURS.length)] : 0xffffff;
    const user = CHAT_USERS[Math.floor(random() * CHAT_USERS.length)];
    rows.push(
      // Mode 1 (scrolling) only: pinned top comments are drawn across the
      // top scrolling row and read as overlapping text.
      `<d p="${t.toFixed(3)},1,25,${colour},${1763150000000 + Math.round(t * 1000)},0,u${i % 97},${i}" user="${escapeXml(user)}">${escapeXml(text)}</d>`,
    );
  }
  return `<?xml version="1.0" encoding="UTF-8"?>\n<i>\n<chatserver>chat.example.com</chatserver>\n${rows.join('\n')}\n</i>\n`;
}

/**
 * Writes every session's thumbnail and, for the featured sessions, a playable
 * video and chat XML for each part. `page` is a CDP page used to render the
 * scenes; `playable` is false when no player shot needs the videos.
 */
export async function writeMedia({ page, sessions, workDir, playable }) {
  await page.send('Emulation.setDeviceMetricsOverride', {
    width: WIDTH,
    height: HEIGHT,
    deviceScaleFactor: 1,
    mobile: false,
  });
  for (const session of sessions) {
    const streamer = STREAMERS.find((s) => s.id === session.streamerId);
    await mkdir(dirname(session.thumbnail), { recursive: true });
    await writeFile(session.thumbnail, await renderScene(page, scene(streamer, session), 'jpeg'));
  }
  if (!playable) return;

  for (const streamerId of new Set(Object.values(FEATURED))) {
    const streamer = STREAMERS.find((s) => s.id === streamerId);
    const session = sessions.find((s) => s.streamerId === streamerId);
    const still = `${workDir}/scene-${streamerId}.png`;
    await writeFile(still, await renderScene(page, scene(streamer, session, { overlay: true }), 'png'));

    const [first, ...rest] = session.parts;
    await mkdir(dirname(first.video), { recursive: true });
    await run('ffmpeg', [
      '-y', '-v', 'error',
      '-loop', '1', '-framerate', '30', '-i', still,
      '-f', 'lavfi', '-i', 'anullsrc=r=48000:cl=stereo',
      '-t', String(SECONDS),
      '-c:v', 'libx264', '-preset', 'veryfast', '-tune', 'stillimage', '-pix_fmt', 'yuv420p',
      '-c:a', 'aac', '-shortest', '-movflags', '+faststart', first.video,
    ]);
    const xml = danmuXml(streamer);
    await writeFile(first.xml, xml);
    for (const part of rest) {
      await copyFile(first.video, part.video);
      await writeFile(part.xml, xml);
    }
    await rm(still, { force: true });
  }
}
