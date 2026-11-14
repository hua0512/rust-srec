// Minimal Chrome DevTools Protocol client over Node's built-in WebSocket,
// so the showcase scripts need no npm dependencies.
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function launchChromium({ binary, port }) {
  const profile = await mkdtemp(join(tmpdir(), 'srec-showcase-chrome-'));
  const proc = spawn(
    binary,
    [
      '--headless=new',
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--hide-scrollbars',
      '--font-render-hinting=none',
      '--force-color-profile=srgb',
      // The player shot starts playback from a script, without a click.
      '--autoplay-policy=no-user-gesture-required',
      // Headless reports no hover-capable pointer, which disables every
      // Tailwind `hover:` style; pretend to have a mouse.
      '--blink-settings=primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4',
      'about:blank',
    ],
    { stdio: 'ignore' },
  );
  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) {
    try {
      await fetch(`${base}/json/version`);
      break;
    } catch {
      await sleep(100);
    }
  }
  return {
    async newPage() {
      const res = await fetch(`${base}/json/new?about:blank`, { method: 'PUT' });
      const { webSocketDebuggerUrl } = await res.json();
      return connect(webSocketDebuggerUrl);
    },
    async close() {
      proc.kill();
      await new Promise((r) => proc.once('exit', r));
      await rm(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
    },
  };
}

async function connect(url) {
  const ws = new WebSocket(url);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let nextId = 1;
  const pending = new Map();
  const listeners = new Set();
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(`${msg.error.message} (${msg.error.code})`));
      else resolve(msg.result);
    } else if (msg.method) {
      for (const fn of listeners) fn(msg);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });

  const page = {
    send,
    on(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    async eval(expression) {
      const { result, exceptionDetails } = await send('Runtime.evaluate', {
        expression,
        awaitPromise: true,
        returnByValue: true,
      });
      if (exceptionDetails) {
        throw new Error(
          exceptionDetails.exception?.description ?? exceptionDetails.text,
        );
      }
      return result.value;
    },
    async waitFor(expression, { timeout = 20000, interval = 100 } = {}) {
      const deadline = Date.now() + timeout;
      for (;;) {
        try {
          if (await page.eval(expression)) return;
        } catch {
          // The page may be mid-navigation; keep polling.
        }
        if (Date.now() > deadline) {
          throw new Error(`Timed out waiting for: ${expression}`);
        }
        await sleep(interval);
      }
    },
    async goto(url) {
      await send('Page.navigate', { url });
      await page.waitFor(`document.readyState === 'complete'`);
    },
    close: () => ws.close(),
  };
  await send('Page.enable');
  await send('Runtime.enable');
  return page;
}

export { sleep };
