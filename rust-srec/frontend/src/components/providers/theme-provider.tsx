import {
  createContext,
  use,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
} from 'react';
import { ThemeSettingsSync } from '@/components/providers/theme-settings-sync';
import {
  DEFAULT_MODE,
  STORAGE_KEY_MODE,
  COOKIE_KEY_MODE,
  COOKIE_MAX_AGE,
  PREFERS_DARK_MEDIA,
  getSystemTheme,
  isMode,
  type Mode,
  type ResolvedMode,
} from '@/lib/theme-config';

const isServer = typeof window === 'undefined';

// useLayoutEffect warns when rendered on the server; the server never applies
// DOM effects anyway, so fall back to useEffect there.
const useIsomorphicLayoutEffect = isServer ? useEffect : useLayoutEffect;

type ThemeProviderProps = {
  children: React.ReactNode;
  serverMode?: Mode;
};

type ThemeProviderState = {
  mode: Mode;
  resolvedMode: ResolvedMode;
  setMode: (mode: Mode) => void;
};

const initialState: ThemeProviderState = {
  mode: 'system',
  resolvedMode: 'light',
  setMode: () => null,
};

const ThemeProviderContext = createContext<ThemeProviderState>(initialState);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function readStoredMode(fallback: Mode): Mode {
  if (isServer) return fallback;
  try {
    const stored = localStorage.getItem(STORAGE_KEY_MODE);
    return isMode(stored) ? stored : fallback;
  } catch {
    return fallback;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // localStorage unavailable
  }
}

function writeCookie(key: string, value: string): void {
  document.cookie = `${key}=${encodeURIComponent(value)};path=/;max-age=${COOKIE_MAX_AGE};samesite=lax`;
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

export function ThemeProvider({ children, serverMode }: ThemeProviderProps) {
  const [mode, setModeState] = useState<Mode>(() =>
    // With SSR (serverMode set, web build) the first client render must match
    // the server markup, so state starts from the cookie-derived mode and the
    // reconcile effect below adopts localStorage right after mount. Without
    // SSR (desktop SPA) localStorage is read directly.
    serverMode !== undefined ? serverMode : readStoredMode(DEFAULT_MODE),
  );
  const [systemTheme, setSystemTheme] = useState<ResolvedMode>(getSystemTheme);

  const resolvedMode: ResolvedMode = mode === 'system' ? systemTheme : mode;

  // --------------------------------------------------
  // Apply theme to <html>
  // --------------------------------------------------

  const applyToDOM = useCallback((resolved: ResolvedMode) => {
    const el = document.documentElement;

    el.classList.remove('light', 'dark');
    el.classList.add(resolved);
    el.style.colorScheme = resolved;
  }, []);

  // --------------------------------------------------
  // Post-hydration reconciliation (SSR path only)
  // --------------------------------------------------

  // State was initialized from the cookie-derived serverMode so hydration
  // matches the server markup. localStorage is the client authority: adopt it
  // now, and repair the cookie when the two stores diverged (themeMiddleware
  // refreshes whatever cookie it sees on every request, so a stale cookie
  // never heals by itself). Layout effect, declared BEFORE the applyToDOM
  // effect below: its setModeState commits before the browser paints, so a
  // stale cookie mode is never applied to <html> post-paint (the pre-paint
  // script already applied the stored mode). Mount-only: setMode keeps both
  // stores written from here on.
  useIsomorphicLayoutEffect(() => {
    if (serverMode === undefined) return;
    const stored = readStoredMode(serverMode);
    if (stored !== mode) setModeState(stored);
    if (stored !== serverMode) writeCookie(COOKIE_KEY_MODE, stored);
  }, []);

  // Apply on every resolvedMode change (including mount).
  // The blocking script in <head> already sets the correct class before
  // React hydrates, and setMode mutates <html> directly, so this effect is
  // the idempotent reconciler for the storage-event and media-query paths.
  // Layout timing keeps every write pre-paint, including the mount-time
  // cookie-value write that the reconcile effect above supersedes.
  useIsomorphicLayoutEffect(() => {
    applyToDOM(resolvedMode);
  }, [resolvedMode, applyToDOM]);

  // --------------------------------------------------
  // Setter: update state + persist
  // --------------------------------------------------

  const setMode = useCallback(
    (next: Mode) => {
      setModeState(next);
      // Mutate <html> synchronously so a document.startViewTransition callback
      // wrapping setMode (use-circular-transition.ts) captures the new theme
      // without depending on when React flushes the applyToDOM effect.
      applyToDOM(next === 'system' ? getSystemTheme() : next);
      writeStorage(STORAGE_KEY_MODE, next);
      writeCookie(COOKIE_KEY_MODE, next);
    },
    [applyToDOM],
  );

  // --------------------------------------------------
  // OS preference listener
  // --------------------------------------------------

  useEffect(() => {
    const media = window.matchMedia(PREFERS_DARK_MEDIA);
    const handler = (e: MediaQueryListEvent) => {
      setSystemTheme(e.matches ? 'dark' : 'light');
    };
    media.addEventListener('change', handler);
    return () => media.removeEventListener('change', handler);
  }, []);

  // --------------------------------------------------
  // Cross-tab sync via storage events
  // --------------------------------------------------

  useEffect(() => {
    const handler = (e: StorageEvent) => {
      if (e.key !== STORAGE_KEY_MODE) return;
      setModeState(isMode(e.newValue) ? e.newValue : DEFAULT_MODE);
    };
    window.addEventListener('storage', handler);
    return () => window.removeEventListener('storage', handler);
  }, []);

  // --------------------------------------------------
  // Context value
  // --------------------------------------------------

  const value = useMemo<ThemeProviderState>(
    () => ({ mode, resolvedMode, setMode }),
    [mode, resolvedMode, setMode],
  );

  return (
    <ThemeProviderContext value={value}>
      <ThemeSettingsSync />
      {children}
    </ThemeProviderContext>
  );
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export const useTheme = () => use(ThemeProviderContext);
