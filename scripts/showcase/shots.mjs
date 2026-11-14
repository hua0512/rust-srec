// Pages to capture. `path` may be a function of the seeded data and the
// locale. `ready` is a JS expression polled until true; `prepare` runs once
// before the capture (async function body, with `sleep(ms)` available) and may
// be a function of the locale. `title` maps each locale to the window title.

export const LOCALES = ['en', 'zh-CN'];

// UI text the scripts below match on, as the frontend translates it.
const TEXT = {
  en: { danmuStatistics: 'Danmu Statistics' },
  'zh-CN': { danmuStatistics: '弹幕统计' },
};

// The chat statistics shot shows a stream in the page's language.
const DANMU_STREAMER = { en: 'aurora', 'zh-CN': 'nightjar' };
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
    height: 1000,
    ready: `${loaded('main [data-slot=card]')} && document.body.innerText.includes('KB/s')`,
  },
  {
    name: 'streamers',
    path: '/streamers',
    title: { en: 'Streamers', 'zh-CN': '主播' },
    ready: `${loaded('main [data-slot=card]')} && document.body.innerText.includes('KB/s')`,
  },
  {
    name: 'sessions',
    path: '/sessions',
    title: { en: 'Sessions', 'zh-CN': '录制会话' },
    ready: loaded('main [data-slot=card]'),
  },
  {
    name: 'danmu-stats',
    path: (seed, locale) =>
      `/sessions/${seed.sessions.find((s) => s.streamerId === DANMU_STREAMER[locale]).id}`,
    title: { en: 'Session', 'zh-CN': '会话' },
    ready: loaded('main svg.recharts-surface'),
    prepare: (locale) => scrollToHeading(TEXT[locale].danmuStatistics),
    settle: 2500,
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
    path: '/pipeline/jobs',
    title: { en: 'Jobs', 'zh-CN': '处理任务' },
    ready: loaded('main'),
  },
];
