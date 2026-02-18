import { DEFAULT_MODE, STORAGE_KEY_MODE } from '@/lib/theme-config';

/**
 * localStorage key where ThemeSettingsSync caches the last-applied
 * CSS variable map so the blocking script can restore them instantly.
 */
export const THEME_VARS_CACHE_KEY = 'theme-vars-cache';

/**
 * Blocking inline script that runs before first paint.
 * This function is .toString()'d and injected as an IIFE — it must be
 * entirely self-contained (no imports, no closures).
 *
 * It does two things:
 * 1. Sets the dark/light class + color-scheme on <html>
 * 2. Restores cached CSS variables from localStorage to avoid a flash
 *    when the user has a non-default theme preset
 */
function themeScript(
  storageKey: string,
  defaultMode: string,
  varsCacheKey: string,
) {
  var el = document.documentElement;

  function getSystemTheme() {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
  }

  try {
    // 1. Resolve and apply dark/light mode
    var mode = localStorage.getItem(storageKey) || defaultMode;
    var resolved = mode === 'system' ? getSystemTheme() : mode;

    el.classList.remove('light', 'dark');
    el.classList.add(resolved);
    el.style.colorScheme = resolved;

    // 2. Restore cached CSS variable map (preset / imported theme vars)
    var cached = localStorage.getItem(varsCacheKey);
    if (cached) {
      var entry = JSON.parse(cached);
      // entry = { light: { [varName]: value }, dark: { ... } }
      var vars = resolved === 'dark' ? entry.dark : entry.light;
      if (vars) {
        for (var key in vars) {
          if (Object.prototype.hasOwnProperty.call(vars, key)) {
            el.style.setProperty('--' + key, vars[key]);
          }
        }
      }
    }
  } catch {
    // localStorage unavailable or malformed cache — fall back to stylesheet defaults
    var fallback = defaultMode === 'system' ? getSystemTheme() : defaultMode;
    el.classList.remove('light', 'dark');
    el.classList.add(fallback);
    el.style.colorScheme = fallback;
  }
}

/**
 * Build the inline script string for injection into <head>.
 */
export function buildThemeScriptHTML(): string {
  const args = JSON.stringify([
    STORAGE_KEY_MODE,
    DEFAULT_MODE,
    THEME_VARS_CACHE_KEY,
  ]);

  return `(${themeScript.toString()}).apply(null,${args})`;
}
