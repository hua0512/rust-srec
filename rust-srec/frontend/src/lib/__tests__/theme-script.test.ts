import {
  buildThemeScriptHTML,
  THEME_CSS_CACHE_KEY,
  USER_THEME_STYLE_ID,
} from '../theme-script';

const TEST_BUILD_ID = 'test-build';

/** Execute the blocking script exactly as a browser would: as a global-scope
 *  classic script (the IIFE string injected into <head>). The subject under
 *  test IS a script string, so Function is the only way to run it globally. */
function runScript() {
  // oxlint-disable-next-line no-implied-eval
  new Function(buildThemeScriptHTML(TEST_BUILD_ID))();
}

function mockSystemDark(dark: boolean) {
  vi.mocked(window.matchMedia).mockImplementation((query: string) => ({
    matches: dark,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}

beforeEach(() => {
  document.documentElement.className = '';
  document.documentElement.style.colorScheme = '';
  document.getElementById(USER_THEME_STYLE_ID)?.remove();
  localStorage.clear();
  mockSystemDark(false);
});

describe('themeScript (blocking pre-paint script)', () => {
  it('applies a stored explicit mode', () => {
    localStorage.setItem('theme', 'dark');

    runScript();

    expect(document.documentElement).toHaveClass('dark');
    expect(document.documentElement).not.toHaveClass('light');
    expect(document.documentElement.style.colorScheme).toBe('dark');
  });

  it('resolves system mode through matchMedia', () => {
    localStorage.setItem('theme', 'system');
    mockSystemDark(true);

    runScript();

    expect(document.documentElement).toHaveClass('dark');
  });

  it('falls back to the default mode for garbage stored values', () => {
    localStorage.setItem('theme', 'purple');

    runScript();

    // default mode is 'system'; the mocked OS preference is light
    expect(document.documentElement).toHaveClass('light');
    expect(document.documentElement).not.toHaveClass('purple');
    expect(document.documentElement.style.colorScheme).toBe('light');
  });

  it('injects the cached user theme when the build id matches', () => {
    const css = ':root{--primary:red;}\n.dark{--primary:blue;}';
    localStorage.setItem(
      THEME_CSS_CACHE_KEY,
      JSON.stringify({ v: TEST_BUILD_ID, css }),
    );

    runScript();

    const el = document.getElementById(USER_THEME_STYLE_ID);
    expect(el).not.toBeNull();
    expect(el?.tagName).toBe('STYLE');
    expect(el?.textContent).toBe(css);
    expect(document.head.contains(el!)).toBe(true);
  });

  it('never creates a second style element when executed twice', () => {
    // The script runs once during parse and again when the router evaluates
    // the root route's head() scripts after hydration. A second element would
    // sit later in <head> and shadow every update applyThemeSideEffects makes
    // to the first via getElementById.
    const css = ':root{--primary:red;}';
    localStorage.setItem(
      THEME_CSS_CACHE_KEY,
      JSON.stringify({ v: TEST_BUILD_ID, css }),
    );

    runScript();
    const first = document.getElementById(USER_THEME_STYLE_ID);
    runScript();

    const all = document.querySelectorAll(`[id="${USER_THEME_STYLE_ID}"]`);
    expect(all).toHaveLength(1);
    expect(all[0]).toBe(first);
    expect(all[0].textContent).toBe(css);
  });

  it('leaves an existing style element untouched on re-execution', () => {
    // Once the app is running, ThemeSettingsSync owns the element; a script
    // re-execution must not overwrite live-applied css with the cached copy.
    const live = document.createElement('style');
    live.id = USER_THEME_STYLE_ID;
    live.textContent = ':root{--primary:live;}';
    document.head.appendChild(live);
    localStorage.setItem(
      THEME_CSS_CACHE_KEY,
      JSON.stringify({ v: TEST_BUILD_ID, css: ':root{--primary:stale;}' }),
    );

    runScript();

    const all = document.querySelectorAll(`[id="${USER_THEME_STYLE_ID}"]`);
    expect(all).toHaveLength(1);
    expect(all[0].textContent).toBe(':root{--primary:live;}');
  });

  it('skips the cache when the build id does not match', () => {
    localStorage.setItem(
      THEME_CSS_CACHE_KEY,
      JSON.stringify({ v: 'older-build', css: ':root{--primary:red;}' }),
    );

    runScript();

    expect(document.getElementById(USER_THEME_STYLE_ID)).toBeNull();
    // mode application must still have happened
    expect(document.documentElement).toHaveClass('light');
  });

  it('ignores a malformed cache without throwing', () => {
    localStorage.setItem('theme', 'dark');
    localStorage.setItem(THEME_CSS_CACHE_KEY, '{not json');

    expect(() => runScript()).not.toThrow();

    expect(document.getElementById(USER_THEME_STYLE_ID)).toBeNull();
    expect(document.documentElement).toHaveClass('dark');
  });
});
