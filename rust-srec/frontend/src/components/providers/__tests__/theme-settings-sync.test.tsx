import { act, render } from '@testing-library/react';

import { ThemeSettingsSync } from '../theme-settings-sync';
import {
  LEGACY_VARS_CACHE_KEY,
  THEME_CSS_CACHE_KEY,
  USER_THEME_STYLE_ID,
} from '@/lib/theme-script';
import {
  DEFAULT_SETTINGS,
  STORAGE_KEY_SETTINGS,
  useThemeSettings,
} from '@/store/theme-settings';

function seedPersistedSettings(partial: Record<string, unknown>) {
  localStorage.setItem(
    STORAGE_KEY_SETTINGS,
    JSON.stringify({ state: { ...DEFAULT_SETTINGS, ...partial }, version: 0 }),
  );
}

function styleEl() {
  return document.getElementById(USER_THEME_STYLE_ID);
}

beforeEach(() => {
  localStorage.clear();
  styleEl()?.remove();
  // Reset to pristine defaults. The module-level store subscription fires on
  // this setState and clears any css cache from a prior test, but localStorage
  // was just cleared anyway.
  useThemeSettings.setState({ ...DEFAULT_SETTINGS });

  const rootStyle = document.documentElement.style;
  for (let i = rootStyle.length - 1; i >= 0; i--) {
    rootStyle.removeProperty(rootStyle[i]);
  }
});

describe('ThemeSettingsSync', () => {
  it('creates the user-theme style element for a non-default preset', () => {
    seedPersistedSettings({ preset: 'black' });

    render(<ThemeSettingsSync />);

    const el = styleEl();
    expect(el).not.toBeNull();
    const css = el!.textContent ?? '';
    // Over-specific selectors keep the element winning the cascade regardless
    // of its position relative to the styles.css link; :not(.dark) keeps the
    // light block from matching html.dark.
    expect(css.indexOf(':root:root:not(.dark){')).toBeGreaterThanOrEqual(0);
    expect(css.indexOf(':root:root.dark{')).toBeGreaterThan(
      css.indexOf(':root:root:not(.dark){'),
    );
    // 'black' preset light primary
    expect(css).toContain('--primary:oklch(0 0 0)');
    expect(document.head.contains(el)).toBe(true);

    // cache written for the blocking script, stamped with the cache id
    // (getThemeCacheId() falls back to 'dev' under vitest — no define)
    const cache = JSON.parse(localStorage.getItem(THEME_CSS_CACHE_KEY)!);
    expect(cache).toEqual({ v: 'dev', css });
  });

  it('removes the style element and cache when settings are pristine', () => {
    const stale = document.createElement('style');
    stale.id = USER_THEME_STYLE_ID;
    stale.textContent = ':root{--primary:red;}';
    document.head.appendChild(stale);
    localStorage.setItem(
      THEME_CSS_CACHE_KEY,
      JSON.stringify({ v: 'dev', css: stale.textContent }),
    );

    render(<ThemeSettingsSync />);

    expect(styleEl()).toBeNull();
    expect(localStorage.getItem(THEME_CSS_CACHE_KEY)).toBeNull();
  });

  it('strips legacy inline vars but preserves the reveal coordinates', () => {
    const rootStyle = document.documentElement.style;
    rootStyle.setProperty('--primary', 'red');
    rootStyle.setProperty('--x', '10%');
    rootStyle.setProperty('--y', '20%');
    localStorage.setItem(LEGACY_VARS_CACHE_KEY, '{}');

    render(<ThemeSettingsSync />);

    expect(rootStyle.getPropertyValue('--primary')).toBe('');
    expect(rootStyle.getPropertyValue('--x')).toBe('10%');
    expect(rootStyle.getPropertyValue('--y')).toBe('20%');
    expect(localStorage.getItem(LEGACY_VARS_CACHE_KEY)).toBeNull();
  });

  it('applies overrides and radius on top of the preset in both blocks', () => {
    seedPersistedSettings({
      radius: '1rem',
      overrides: { primary: '#123456' },
    });

    render(<ThemeSettingsSync />);

    const css = styleEl()!.textContent ?? '';
    const [rootBlock, darkBlock] = css.split(':root:root.dark{');
    for (const block of [rootBlock, darkBlock]) {
      expect(block).toContain('--radius:1rem;');
      expect(block).toContain('--primary:#123456;');
    }
  });

  it('keeps asymmetric imported themes out of the opposite mode', () => {
    // import-modal.tsx parses :root and .dark sections independently, so an
    // import can define a var in one mode only. The light block must not
    // match html.dark (mutually exclusive selectors), or this light-only
    // --primary would override the stylesheet's .dark fallback in dark mode.
    seedPersistedSettings({
      base: 'imported',
      importedTheme: { light: { primary: 'red' }, dark: {} },
    });

    render(<ThemeSettingsSync />);

    const css = styleEl()!.textContent ?? '';
    const [lightBlock, darkBlock] = css.split(':root:root.dark{');
    expect(lightBlock).toContain(':root:root:not(.dark){');
    expect(lightBlock).toContain('--primary:red;');
    expect(darkBlock).not.toContain('--primary');
  });

  it('rehydrates when another tab writes the settings key', () => {
    render(<ThemeSettingsSync />);
    expect(styleEl()).toBeNull(); // pristine

    seedPersistedSettings({ preset: 'black' });
    act(() => {
      window.dispatchEvent(
        new StorageEvent('storage', { key: STORAGE_KEY_SETTINGS }),
      );
    });

    expect(styleEl()).not.toBeNull();
    expect(styleEl()!.textContent).toContain('--primary:oklch(0 0 0)');
  });

  it('updates the css cache synchronously inside a store setter (no effect flush)', () => {
    // The blocking script reads theme-css-cache on the next load; it must never
    // observe new settings paired with stale css. A setter writes both
    // theme-settings (zustand persist) and the cache within the same setState,
    // so the cache reflects the change before any React effect runs.
    render(<ThemeSettingsSync />);
    expect(localStorage.getItem(THEME_CSS_CACHE_KEY)).toBeNull(); // pristine

    // Deliberately NOT wrapped in act(): asserts the cache is already current
    // synchronously, with no effect/microtask flush in between.
    useThemeSettings.getState().setPreset('black');

    const cache = JSON.parse(localStorage.getItem(THEME_CSS_CACHE_KEY)!);
    expect(cache.v).toBe('dev');
    expect(cache.css).toContain('--primary:oklch(0 0 0)');
  });

  it('clears the css cache synchronously when settings return to pristine', () => {
    seedPersistedSettings({ preset: 'black' });
    render(<ThemeSettingsSync />);
    expect(localStorage.getItem(THEME_CSS_CACHE_KEY)).not.toBeNull();

    useThemeSettings.getState().reset();

    expect(localStorage.getItem(THEME_CSS_CACHE_KEY)).toBeNull();
  });
});
