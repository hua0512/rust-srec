// Imported relatively (not via '@/') because vite.desktop.config.ts also
// imports this module to inject the script into the desktop HTML entries,
// and the '@' alias does not resolve while the config itself is bundled.
import { DEFAULT_MODE, STORAGE_KEY_MODE } from './theme-config';

/**
 * localStorage key holding `{ v: buildId, css: string }` — the compiled
 * user-theme stylesheet ThemeSettingsSync derives from the settings store,
 * cached so the blocking script can restore it before first paint.
 */
export const THEME_CSS_CACHE_KEY = 'theme-css-cache';

/**
 * id of the <style> element carrying the user theme (`:root{...}.dark{...}`).
 * Created by the blocking script from the cache, then owned/replaced by
 * ThemeSettingsSync after hydration.
 */
export const USER_THEME_STYLE_ID = 'user-theme-vars';

/**
 * Cache key of the retired inline-style token system; ThemeSettingsSync
 * removes it once on mount.
 */
export const LEGACY_VARS_CACHE_KEY = 'theme-vars-cache';

/**
 * Identity stamped into the css cache. A mismatch makes the blocking script
 * skip the cache and fall back to stylesheet defaults, instead of replaying
 * css compiled from outdated preset data or compile logic. `__THEME_CACHE_ID__`
 * comes from `define` in both Vite configs (a content hash of the theming
 * sources — see theme-cache-id.ts), so it survives releases that do not touch
 * theming. The fallback covers vitest and node-side imports of this module.
 */
export function getThemeCacheId(): string {
  return typeof __THEME_CACHE_ID__ === 'string' ? __THEME_CACHE_ID__ : 'dev';
}

/**
 * Pre-stylesheet background so a frame painted before styles.css loads is
 * never white in dark mode (matters on desktop, where styles.css is a bundled
 * asset rather than a render-blocking SSR <link>). Values duplicate
 * `--background` in styles.css :root/.dark — guarded by
 * theme-default-drift.test.ts.
 */
export const CRITICAL_THEME_CSS =
  'html{background-color:oklch(1 0 0)}html.dark{background-color:oklch(0.141 0.005 285.823)}';

/**
 * Blocking inline script that runs before first paint.
 * This function is .toString()'d and injected as an IIFE — it must be
 * entirely self-contained (no imports, no closures).
 *
 * It does two things:
 * 1. Sets the dark/light class + color-scheme on <html> (validated mode)
 * 2. Injects the cached user-theme <style> element so a non-default
 *    preset/imported theme is present at first paint. Both light and dark
 *    blocks are in the css, so a pre-hydration OS scheme flip stays correct.
 */
function themeScript(
  storageKey: string,
  defaultMode: string,
  cacheKey: string,
  styleId: string,
  cacheId: string,
) {
  var el = document.documentElement;
  var modes = ['light', 'dark', 'system'];

  function systemTheme() {
    try {
      return window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light';
    } catch {
      return 'light';
    }
  }

  var mode = defaultMode;
  try {
    var stored = localStorage.getItem(storageKey);
    if (stored && modes.indexOf(stored) !== -1) mode = stored;
  } catch {
    // localStorage unavailable — keep defaultMode
  }

  var resolved = mode === 'system' ? systemTheme() : mode;
  el.classList.remove('light', 'dark');
  el.classList.add(resolved);
  el.style.colorScheme = resolved;

  try {
    // This script runs more than once per document: as the parse-time inline
    // script from the server/desktop HTML, and again when the router
    // evaluates the root route's head() scripts after hydration. Once the
    // element exists, ThemeSettingsSync owns it (applyThemeSideEffects
    // updates it in place via getElementById) — creating a second element
    // here would sit later in <head> and, with equal specificity, shadow
    // every live update made to the first one.
    if (!document.getElementById(styleId)) {
      var raw = localStorage.getItem(cacheKey);
      if (raw) {
        var entry = JSON.parse(raw);
        if (entry && entry.v === cacheId && typeof entry.css === 'string') {
          var style = document.createElement('style');
          style.id = styleId;
          style.textContent = entry.css;
          // Position in <head> does not matter: the cached css uses
          // over-specific selectors (:root:root — see compileThemeCss in
          // theme-settings-sync.tsx) precisely because the styles.css <link>
          // may not exist in the DOM yet while this runs during parse.
          document.head.appendChild(style);
        }
      }
    }
  } catch {
    // malformed cache — stylesheet defaults still apply
  }
}

/**
 * Build the inline script string for injection into <head>.
 * `cacheId` is overridable so vite.desktop.config.ts can pass the exact value
 * it also sets as the `__THEME_CACHE_ID__` define (they must match, or the
 * cache written by ThemeSettingsSync never validates in this script).
 */
export function buildThemeScriptHTML(
  cacheId: string = getThemeCacheId(),
): string {
  const args = JSON.stringify([
    STORAGE_KEY_MODE,
    DEFAULT_MODE,
    THEME_CSS_CACHE_KEY,
    USER_THEME_STYLE_ID,
    cacheId,
  ]);

  return `(${themeScript.toString()}).apply(null,${args})`;
}
