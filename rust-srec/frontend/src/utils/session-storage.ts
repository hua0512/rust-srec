import type { SessionData } from './session';

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
    if (
      typeof parsed !== 'object' ||
      parsed === null ||
      Array.isArray(parsed)
    ) {
      return {};
    }
    return parsed as Partial<SessionData>;
  } catch {
    return {};
  }
}
