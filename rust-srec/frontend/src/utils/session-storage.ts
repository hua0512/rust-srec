import type { SessionData } from './session';

// Single source of truth for the localStorage-backed session used by desktop
// (Tauri SPA) builds. utils/session.ts (getDesktopAccessToken) and
// utils/session.server.ts (getBrowserSession) both read/write this key, so
// the key and parse logic live here to keep the two readers in lockstep.
export const BROWSER_SESSION_STORAGE_KEY = 'rust_srec_session_v1';

export function isBrowserRuntime(): boolean {
  return (
    typeof window !== 'undefined' && typeof window.localStorage !== 'undefined'
  );
}

export function parseStoredSession(raw: string | null): Partial<SessionData> {
  if (!raw) return {};

  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== 'object' || parsed === null) return {};
    return parsed as Partial<SessionData>;
  } catch {
    return {};
  }
}
