export const MODES = ['light', 'dark', 'system'] as const;
export type Mode = (typeof MODES)[number];
export type ResolvedMode = Exclude<Mode, 'system'>;

export const DEFAULT_MODE: Mode = 'system';

export const STORAGE_KEY_MODE = 'theme';
export const COOKIE_KEY_MODE = 'theme';
export const COOKIE_MAX_AGE = 60 * 60 * 24 * 365; // 1 year

export const PREFERS_DARK_MEDIA = '(prefers-color-scheme: dark)';

/** Guard for values read back from localStorage, cookies, or storage events —
 *  a corrupted value must never reach `classList.add` / `color-scheme`. */
export function isMode(value: unknown): value is Mode {
  return MODES.includes(value as Mode);
}

export function getSystemTheme(): ResolvedMode {
  if (typeof window === 'undefined') return 'light';
  return window.matchMedia(PREFERS_DARK_MEDIA).matches ? 'dark' : 'light';
}
