#!/usr/bin/env node
// Captures framed showcase screenshots of the web UI with fictional data.
// See README.md in this directory for usage.
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

import { launchChromium, sleep } from './cdp.mjs';
import { startProxy } from './proxy.mjs';
import { writeMedia } from './media.mjs';
import { createJobs, seedDatabase } from './seed.mjs';
import { mintSessionCookie } from './session.mjs';
import { LOCALES, PHONE, SHOTS } from './shots.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '../..');
const frontendDir = join(repo, 'rust-srec/frontend');

const { values: opts } = parseArgs({
  options: {
    backend: { type: 'string', default: join(repo, 'target/debug/rust-srec') },
    chromium: { type: 'string', default: 'chromium' },
    out: { type: 'string', default: join(repo, 'rust-srec/docs/public/images/showcase') },
    theme: { type: 'string', default: 'dark,light' },
    locale: { type: 'string', default: LOCALES.join(',') },
    only: { type: 'string' },
    raw: { type: 'boolean', default: false },
    keep: { type: 'boolean', default: false },
  },
});
const themes = opts.theme.split(',');
const locales = opts.locale.split(',');
for (const locale of locales) {
  if (!LOCALES.includes(locale)) {
    throw new Error(`unknown locale ${locale}; expected one of ${LOCALES.join(', ')}`);
  }
}
const want = (name) => !opts.only || opts.only.split(',').includes(name);
const shots = SHOTS.filter((s) => want(s.name));

const SESSION_SECRET = 'showcase-session-secret-not-for-production-use';
const children = [];
let workDir;
let browser;

function freePort() {
  return new Promise((res) => {
    const srv = net.createServer().listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => res(port));
    });
  });
}

async function waitForHttp(url, timeout = 120_000) {
  const deadline = Date.now() + timeout;
  for (;;) {
    try {
      const res = await fetch(url);
      if (res.status < 500) return;
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) throw new Error(`Timed out waiting for ${url}`);
    await sleep(300);
  }
}

function startProcess(cmd, args, { env, cwd, log }) {
  const child = spawn(cmd, args, {
    cwd,
    env: Object.fromEntries(
      Object.entries({ ...process.env, ...env }).filter(([, v]) => v !== undefined),
    ),
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const chunks = [];
  child.stdout.on('data', (c) => chunks.push(c));
  child.stderr.on('data', (c) => chunks.push(c));
  child.on('exit', () => writeFile(log, Buffer.concat(chunks)).catch(() => {}));
  children.push(child);
  return child;
}

async function stopProcess(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGTERM');
  await Promise.race([new Promise((r) => child.once('exit', r)), sleep(15_000)]);
  if (child.exitCode === null) child.kill('SIGKILL');
}

async function startBackend(work, port, vitePort) {
  const child = startProcess(opts.backend, [], {
    log: join(work, `backend-${Date.now()}.log`),
    env: {
      AUTH_DISABLED: 'true',
      // AUTH_DISABLED only takes effect without a JWT secret.
      JWT_SECRET: undefined,
      API_BIND_ADDRESS: '127.0.0.1',
      API_PORT: String(port),
      DATABASE_URL: `sqlite:${join(work, 'srec.db')}?mode=rwc`,
      OUTPUT_DIR: join(work, 'recordings'),
      LOG_DIR: join(work, 'logs'),
      RUST_SREC_LOCALE: 'en',
      // The player fetches chat files from the API with script, so the dev
      // server's origin must be allowed.
      API_CORS_ORIGINS: `http://localhost:${vitePort},http://127.0.0.1:${vitePort}`,
    },
  });
  await waitForHttp(`http://127.0.0.1:${port}/api/health`);
  return child;
}

// Hides dev-only overlays and scrollbars, and stops caret blink and
// animations so every capture settles on the same frame.
const CAPTURE_CSS = `
  .tsqd-parent-container, [class*="tanstack-devtools"], #tanstack-devtools,
  [data-testid="tanstack_devtools"] { display: none !important; }
  ::-webkit-scrollbar { display: none !important; }
  * { caret-color: transparent !important; }
`;

async function capturePage(page, base, shot, theme, locale, seed) {
  const resolve = (value) => (typeof value === 'function' ? value({ seed, locale }) : value);
  const { width = 1280, height = 800, mobile = false } = shot;
  await page.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: mobile ? 3 : 2,
    mobile,
  });
  await page.send('Emulation.setEmulatedMedia', {
    features: [{ name: 'prefers-color-scheme', value: theme }],
  });
  // The frontend reads its language from this cookie; the override makes
  // Intl date and number formatting match it.
  await page.send('Network.setCookie', {
    name: 'app-locale',
    value: locale,
    domain: 'localhost',
    path: '/',
  });
  await page.send('Emulation.setLocaleOverride', { locale });
  // The dashboard layout reads the sidebar state from this cookie on load.
  await page.send('Network.setCookie', {
    name: 'sidebar_state',
    value: String(!shot.collapseSidebar),
    domain: 'localhost',
    path: '/',
  });
  await page.goto(`${base}${resolve(shot.path)}`);
  await page.waitFor(resolve(shot.ready), { timeout: 30_000 });
  await page.eval(`(() => {
    const s = document.createElement('style');
    s.textContent = ${JSON.stringify(CAPTURE_CSS)};
    document.head.append(s);
  })()`);
  const prepare = resolve(shot.prepare);
  if (prepare) await page.eval(`(async () => { ${prepare} })()`);
  // Let charts, lazy images and entry transitions finish.
  // Bounded: lazy images outside the viewport never load.
  await page.eval(
    `Promise.race([
      Promise.all([...document.images].filter(i => !i.complete).map(i => new Promise(r => { i.onload = i.onerror = r; }))),
      new Promise(r => setTimeout(r, 5000)),
    ])`,
  );
  await sleep(shot.settle ?? 1500);
  // `clip` crops the capture to one element, such as a dialog.
  const clip = shot.clip
    ? await page.eval(
        `(() => { const r = document.querySelector(${JSON.stringify(shot.clip)}).getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height, scale: 1 }; })()`,
      )
    : undefined;
  const { data } = await page.send('Page.captureScreenshot', { format: 'png', clip });
  return { png: data, width: clip?.width ?? width };
}

// Loads an HTML template from this directory, runs `script` in it to fill
// in the captures, and screenshots its #stage.
async function render(page, file, { theme = 'dark', width, height, scale = 2, format = 'webp', script }) {
  const html = (await readFile(join(here, file), 'utf8')).replace(
    '<html data-theme="dark">',
    `<html data-theme="${theme}">`,
  );
  const path = join(tmpdir(), `srec-showcase-${process.pid}-${file}`);
  await writeFile(path, html);
  await page.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: scale,
    mobile: false,
  });
  await page.goto(pathToFileURL(path).href);
  await page.eval(`(async () => {
    ${script}
    await Promise.all([...document.images].map((i) => i.decode()));
    await document.fonts.ready;
  })()`);
  const box = await page.eval(
    `(() => { const r = document.getElementById('stage').getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; })()`,
  );
  const { data } = await page.send('Page.captureScreenshot', {
    format,
    ...(format === 'webp' ? { quality: 90 } : {}),
    clip: { ...box, scale: 1 },
  });
  await rm(path, { force: true });
  return Buffer.from(data, 'base64');
}

const dataUrl = (png) => `data:image/png;base64,${png}`;

function frameWindow(page, png, shot, theme, locale) {
  const { width = 1280, height = 800 } = shot;
  return render(page, 'frame.html', {
    theme,
    width: width + 400,
    height: height + 400,
    script: `
      document.getElementById('title').textContent = ${JSON.stringify(shot.title?.[locale] ?? 'Rust-srec')};
      const img = document.getElementById('shot');
      img.style.width = '${width}px';
      img.src = ${JSON.stringify(dataUrl(png))};`,
  });
}

function framePhones(page, pngs, theme) {
  return render(page, 'phone.html', {
    theme,
    width: 1600,
    height: 1200,
    script: `
      const stage = document.getElementById('stage');
      const template = document.getElementById('phone');
      for (const src of ${JSON.stringify(pngs.map(dataUrl))}) {
        const phone = template.content.cloneNode(true);
        phone.querySelector('.shot').src = src;
        stage.append(phone);
      }`,
  });
}

function frameSocial(page, png) {
  const logo = pathToFileURL(join(repo, 'rust-srec/docs/public/stream-rec-orange.svg')).href;
  return render(page, 'social.html', {
    width: 1280,
    height: 640,
    scale: 1,
    format: 'png',
    script: `
      document.getElementById('logo').src = ${JSON.stringify(logo)};
      document.getElementById('shot').src = ${JSON.stringify(dataUrl(png))};`,
  });
}

async function main() {
  const work = await mkdtemp(join(tmpdir(), 'srec-showcase-'));
  workDir = work;
  console.log(`work dir: ${work}`);
  const [backendPort, proxyPort, vitePort, cdpPort] = await Promise.all(
    Array.from({ length: 4 }, freePort),
  );

  // First start applies the migrations; seeding needs the schema.
  let backend = await startBackend(work, backendPort, vitePort);
  await stopProcess(backend);
  const seeded = await seedDatabase({
    dbPath: join(work, 'srec.db'),
    outputDir: join(work, 'recordings'),
  });
  backend = await startBackend(work, backendPort, vitePort);
  console.log('backend ready, running pipeline jobs');
  await createJobs({
    apiBase: `http://127.0.0.1:${backendPort}/api`,
    outputDir: join(work, 'recordings'),
    sessions: seeded.sessions,
  });

  const proxy = await startProxy({
    port: proxyPort,
    backendPort,
    liveIds: seeded.liveIds,
    recordings: () => seeded.recordings,
    pathAlias: [work, '/data'],
  });

  const proxyUrl = `http://127.0.0.1:${proxyPort}`;
  // Both URLs are overridden explicitly: the frontend's .env may point at a
  // real deployment.
  startProcess(
    process.execPath,
    [
      join(frontendDir, 'node_modules/vite/bin/vite.js'),
      'dev',
      '--port',
      String(vitePort),
      '--strictPort',
    ],
    {
      cwd: frontendDir,
      log: join(work, 'vite.log'),
      env: {
        BACKEND_URL: proxyUrl,
        VITE_API_BASE_URL: `${proxyUrl}/api`,
        SESSION_SECRET,
      },
    },
  );
  const base = `http://localhost:${vitePort}`;
  await waitForHttp(`${base}/login`);
  console.log('frontend ready');

  browser = await launchChromium({ binary: opts.chromium, port: cdpPort });
  try {
    const page = await browser.newPage();
    await page.send('Network.enable');
    // The dev-only devtools event stream holds a connection open per page.
    await page.send('Network.setBlockedURLs', { urls: ['*/__tsd/*'] });
    await page.send('Network.setCookie', {
      name: 'srec_session',
      value: await mintSessionCookie({ frontendDir, secret: SESSION_SECRET }),
      domain: 'localhost',
      path: '/',
    });
    await writeMedia({
      page,
      sessions: seeded.sessions,
      workDir: work,
      playable: want('player'),
    });

    // Warm up: the first dev-server request compiles most of the app.
    await page.goto(`${base}/dashboard`);
    await sleep(3000);

    for (const locale of locales) {
      const outDir = join(opts.out, locale);
      await mkdir(outDir, { recursive: true });
      for (const shot of shots) {
        for (const theme of themes) {
          const { png, width } = await capturePage(page, base, shot, theme, locale, seeded);
          const name = `${shot.name}-${theme}`;
          if (opts.raw) {
            await writeFile(join(outDir, `${name}.raw.png`), Buffer.from(png, 'base64'));
          }
          const webp = await frameWindow(page, png, { ...shot, width }, theme, locale);
          await writeFile(join(outDir, `${name}.webp`), webp);
          console.log(`wrote ${locale}/${name}.webp`);
        }
      }
      if (want(PHONE.name)) {
        for (const theme of themes) {
          const pngs = [];
          for (const phonePage of PHONE.pages) {
            const shot = { ...PHONE, ...phonePage, mobile: true };
            pngs.push((await capturePage(page, base, shot, theme, locale, seeded)).png);
          }
          const name = `${PHONE.name}-${theme}`;
          await writeFile(join(outDir, `${name}.webp`), await framePhones(page, pngs, theme));
          console.log(`wrote ${locale}/${name}.webp`);
        }
      }
    }

    // The social preview is English only: GitHub and link previews take one
    // image per repository or site.
    if (want('social') && locales.includes('en')) {
      const dashboard = SHOTS.find((s) => s.name === 'dashboard');
      const { png } = await capturePage(page, base, dashboard, 'dark', 'en', seeded);
      await writeFile(join(opts.out, 'social-preview.png'), await frameSocial(page, png));
      console.log('wrote social-preview.png');
    }
    page.close();
  } finally {
    await browser?.close();
    browser = undefined;
    if (opts.keep) {
      console.log(`--keep: frontend ${base}, backend ${backendPort}; Ctrl-C to stop`);
      await new Promise(() => {});
    }
    proxy.close();
    await cleanup();
  }
}

async function cleanup() {
  await browser?.close();
  browser = undefined;
  await Promise.all(children.map(stopProcess));
  if (workDir) await rm(workDir, { recursive: true, force: true });
}

process.on('SIGINT', async () => {
  await cleanup();
  process.exit(130);
});

main().catch(async (err) => {
  console.error(err);
  await cleanup();
  process.exit(1);
});
