import * as React from 'react';

import {
  getThemeCacheId,
  LEGACY_VARS_CACHE_KEY,
  THEME_CSS_CACHE_KEY,
  USER_THEME_STYLE_ID,
} from '@/lib/theme-script';
import { shadcnThemePresets } from '@/utils/shadcn-ui-theme-presets';
import {
  DEFAULT_SETTINGS,
  STORAGE_KEY_SETTINGS,
  useThemeSettings,
  type ThemeSettingsSnapshot,
} from '@/store/theme-settings';

function withSidebarVars(vars: Record<string, string>): Record<string, string> {
  const next = { ...vars };

  const fallbacks: Record<string, string> = {
    sidebar: 'background',
    'sidebar-foreground': 'foreground',
    'sidebar-primary': 'primary',
    'sidebar-primary-foreground': 'primary-foreground',
    'sidebar-accent': 'accent',
    'sidebar-accent-foreground': 'accent-foreground',
    'sidebar-border': 'border',
    'sidebar-ring': 'ring',
  };

  for (const [key, fallbackKey] of Object.entries(fallbacks)) {
    if (next[key] !== undefined) continue;
    const fallbackValue = next[fallbackKey];
    if (fallbackValue !== undefined) {
      next[key] = fallbackValue;
    }
  }

  return next;
}

/**
 * Resolve the full CSS variable map for a given mode (light/dark).
 * `radius` and `overrides` apply identically to both modes.
 */
function resolveVars(
  settings: ThemeSettingsSnapshot,
  isDark: boolean,
): Record<string, string> {
  const baseVars =
    settings.base === 'imported' && settings.importedTheme
      ? isDark
        ? settings.importedTheme.dark
        : settings.importedTheme.light
      : isDark
        ? (shadcnThemePresets[settings.preset] ?? shadcnThemePresets.default)
            .styles.dark
        : (shadcnThemePresets[settings.preset] ?? shadcnThemePresets.default)
            .styles.light;

  return {
    ...withSidebarVars(baseVars),
    ...(settings.radius ? { radius: settings.radius } : {}),
    ...settings.overrides,
  };
}

/**
 * Compile settings into the user-theme stylesheet. The selectors are
 * deliberately over-specific — `:root:root:not(.dark)` and `:root:root.dark`
 * (both 0-3-0) beat the stylesheet's `:root`/`.dark` (0-1-0) — so the element
 * wins the cascade regardless of where it sits relative to the styles.css
 * <link>. That matters because the blocking script inserts it during parse,
 * before Vite's stylesheet link exists in the DOM on desktop. The two
 * selectors are also mutually exclusive: a key present only in the light map
 * (import-modal.tsx parses :root and .dark sections independently, so
 * imported themes can be asymmetric) must not match `html.dark` and override
 * the stylesheet's `.dark` fallback while dark mode is active.
 */
function compileThemeCss(settings: ThemeSettingsSnapshot): string {
  const block = (vars: Record<string, string>) =>
    Object.entries(vars)
      .map(([key, value]) => `--${key}:${value};`)
      .join('');
  return `:root:root:not(.dark){${block(resolveVars(settings, false))}}\n:root:root.dark{${block(resolveVars(settings, true))}}`;
}

/**
 * Pristine settings need no injected stylesheet: styles.css already carries
 * every default-preset value (enforced by theme-default-drift.test.ts).
 */
function isPristine(settings: ThemeSettingsSnapshot): boolean {
  return (
    settings.base === DEFAULT_SETTINGS.base &&
    settings.preset === DEFAULT_SETTINGS.preset &&
    settings.radius === DEFAULT_SETTINGS.radius &&
    Object.keys(settings.overrides).length === 0 &&
    settings.importedTheme === null
  );
}

/**
 * Apply every settings-derived side effect for one snapshot: the live
 * `<style id="user-theme-vars">` element and the pre-paint `theme-css-cache`.
 *
 * Compiles the css exactly once and drives both sinks from it. Runs
 * synchronously from the store subscription below (inside the same zustand
 * `setState` that the persist middleware uses to write `theme-settings`), so:
 *   - the css cache and the persisted settings are always written together —
 *     a reload can never observe new settings paired with stale cached css;
 *   - the live `<style>` element updates without a React render, so dragging
 *     a color picker does not recompile twice or re-render this component.
 */
function applyThemeSideEffects(settings: ThemeSettingsSnapshot): void {
  if (typeof document === 'undefined') return;

  const existing = document.getElementById(USER_THEME_STYLE_ID);

  if (isPristine(settings)) {
    existing?.remove();
    try {
      localStorage.removeItem(THEME_CSS_CACHE_KEY);
    } catch {
      // localStorage unavailable
    }
    return;
  }

  const css = compileThemeCss(settings);

  if (existing) {
    if (existing.textContent !== css) existing.textContent = css;
  } else {
    const el = document.createElement('style');
    el.id = USER_THEME_STYLE_ID;
    el.textContent = css;
    document.head.appendChild(el);
  }

  try {
    localStorage.setItem(
      THEME_CSS_CACHE_KEY,
      JSON.stringify({ v: getThemeCacheId(), css }),
    );
  } catch {
    // full or unavailable — next load falls back to stylesheet defaults
  }
}

// Register once at module load: zustand's persist middleware writes
// `theme-settings` synchronously inside each setter, and subscribers fire
// within that same `setState`, so the css cache stays in lockstep with the
// persisted settings. The store module is imported during SSR, so guard on
// the browser.
if (typeof window !== 'undefined') {
  useThemeSettings.subscribe((state) => applyThemeSideEffects(state));
}

export function ThemeSettingsSync() {
  React.useEffect(() => {
    // One-time migration from the retired inline-style token system: drop its
    // cache key and strip inline --* vars from <html> (left by an old blocking
    // script during a mixed-version deploy window, they would permanently
    // shadow the user-theme <style> element). --x/--y belong to
    // use-circular-transition and must survive.
    try {
      localStorage.removeItem(LEGACY_VARS_CACHE_KEY);
    } catch {
      // localStorage unavailable
    }
    const rootStyle = document.documentElement.style;
    for (let i = rootStyle.length - 1; i >= 0; i--) {
      const prop = rootStyle[i];
      if (prop.startsWith('--') && prop !== '--x' && prop !== '--y') {
        rootStyle.removeProperty(prop);
      }
    }

    void useThemeSettings.persist.rehydrate();
    // rehydrate() runs through setState and thus fires the subscription, but
    // apply once explicitly to cover a pristine session holding a stale cache
    // or a customized session whose cache predates a cache-id change.
    applyThemeSideEffects(useThemeSettings.getState());
  }, []);

  // Cross-tab: re-read persisted settings when another tab writes them —
  // mirrors what theme-provider.tsx does for the mode key. rehydrate() runs
  // through setState, so the subscription refreshes this tab's <style> + cache.
  React.useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY_SETTINGS) {
        void useThemeSettings.persist.rehydrate();
      }
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  return null;
}
