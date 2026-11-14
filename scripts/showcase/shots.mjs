// Pages to capture, at 1280×800 unless `width`/`height` say otherwise, with
// the sidebar collapsed when `collapseSidebar` is set. `ready` is a JS
// expression polled until true; `prepare` runs once before the capture (async
// function body, with `sleep(ms)` available). `path`, `ready` and `prepare`
// may instead be functions of `{ seed, locale }`. `title` maps each locale to
// the window title.
import { FEATURED } from './media.mjs';

export const LOCALES = ['en', 'zh-CN'];

// UI text the scripts below match on, as the frontend translates it.
const TEXT = {
  en: { danmuStatistics: 'Danmu Statistics', play: 'Play', sessionPreview: 'Session Preview' },
  'zh-CN': { danmuStatistics: '弹幕统计', play: '播放', sessionPreview: '录制预览' },
};

// The chat statistics and player shots show a stream in the page's language.
const featuredSession = ({ seed, locale }) =>
  `/sessions/${seed.sessions.find((s) => s.streamerId === FEATURED[locale]).id}`;
const loaded = (selector) =>
  `!!document.querySelector(${JSON.stringify(selector)}) && !document.querySelector('main [data-slot=skeleton]')`;

const PRELUDE = 'const sleep = (ms) => new Promise((r) => setTimeout(r, ms));';

// Scrolls the element whose text is `text` to the top of the viewport.
const scrollToHeading = (text, offset = 96) => `${PRELUDE}
  const el = [...document.querySelectorAll('main h1, main h2, main h3, main [data-slot=card-title]')]
    .find((e) => e.textContent.trim() === ${JSON.stringify(text)});
  if (!el) throw new Error('heading not found: ' + ${JSON.stringify(text)});
  const scroller = (() => {
    for (let p = el.parentElement; p; p = p.parentElement) {
      const o = getComputedStyle(p).overflowY;
      if ((o === 'auto' || o === 'scroll') && p.scrollHeight > p.clientHeight) return p;
    }
    return document.scrollingElement;
  })();
  const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  scroller.scrollTop += top - ${offset};
  await sleep(800);`;

export const SHOTS = [
  {
    name: 'dashboard',
    path: '/dashboard',
    title: { en: 'Dashboard', 'zh-CN': '仪表盘' },
    height: 980,
    ready: `${loaded('main [data-slot=card]')} && document.body.innerText.includes('KB/s')`,
  },
  {
    name: 'streamers',
    path: '/streamers',
    title: { en: 'Streamers', 'zh-CN': '主播' },
    ready: `${loaded('main [data-slot=card]')} && document.body.innerText.includes('KB/s')`,
  },
  {
    name: 'session',
    path: featuredSession,
    title: { en: 'Session', 'zh-CN': '会话' },
    ready: ({ locale }) =>
      `${loaded('main [data-slot=card]')} && !!document.querySelector('main button[aria-label="${TEXT[locale].play}"]')`,
    prepare: ({ locale }) => scrollToHeading(TEXT[locale].sessionPreview),
  },
  {
    name: 'danmu-stats',
    collapseSidebar: true,
    path: featuredSession,
    title: { en: 'Session', 'zh-CN': '会话' },
    ready: loaded('main svg.recharts-surface'),
    prepare: ({ locale }) => scrollToHeading(TEXT[locale].danmuStatistics),
    settle: 2500,
  },
  {
    name: 'player',
    path: featuredSession,
    title: { en: 'Player', 'zh-CN': '播放器' },
    ready: ({ locale }) =>
      `${loaded('main [data-slot=card]')} && !!document.querySelector('main button[aria-label="${TEXT[locale].play}"]')`,
    prepare: ({ locale }) => `${PRELUDE}
      document.querySelector('main button[aria-label="${TEXT[locale].play}"]').click();
      let video;
      for (let i = 0; i < 300; i++) {
        video = document.querySelector('[role=dialog] video');
        if (video && video.readyState >= 3) break;
        await sleep(100);
      }
      if (!video || video.readyState < 3) throw new Error('player video did not load');
      video.muted = true;
      video.currentTime = 4;
      await video.play();
      // Let the chat fill the screen.
      await sleep(6000);
      document.activeElement?.blur();`,
    settle: 0,
    clip: '[role=dialog]',
  },
  {
    name: 'workflow',
    path: '/pipeline/workflows/pipeline-multimedia-archive',
    title: { en: 'Workflow editor', 'zh-CN': '工作流编辑器' },
    ready: loaded('main form, main [data-slot=card]'),
    prepare: `${PRELUDE}
      // Radix tabs activate on mousedown, not click. The trigger id ends in
      // the tab's value, which does not depend on the locale.
      const tab = document.querySelector('main [role=tab][id$="-trigger-graph"]');
      tab.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 }));
      await sleep(1500);`,
  },
  {
    name: 'jobs',
    collapseSidebar: true,
    path: '/pipeline/jobs',
    title: { en: 'Jobs', 'zh-CN': '处理任务' },
    ready: loaded('main'),
  },
];

// Pages captured at phone size and shown side by side in phone frames.
export const PHONE = {
  name: 'mobile',
  width: 390,
  height: 844,
  pages: [
    {
      path: '/dashboard',
      ready: `${loaded('main [data-slot=card]')} && document.body.innerText.includes('KB/s')`,
    },
    {
      path: '/streamers',
      ready: `${loaded('main [data-slot=card]')} && document.body.innerText.includes('KB/s')`,
    },
    { path: '/sessions', ready: loaded('main [data-slot=card]') },
  ],
};
