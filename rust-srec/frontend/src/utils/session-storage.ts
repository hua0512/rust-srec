import type { SessionData } from './session';

export const BROWSER_SESSION_STORAGE_KEY = 'rust_srec_session_v1';

export type DesktopBackendMode = 'local' | 'remote';

// Injected by the desktop shell before the app loads; absent on the web.
interface DesktopGlobals {
  __RUST_SREC_BACKEND_MODE__?: unknown;
  __RUST_SREC_REMOTE_URL__?: unknown;
}

function desktopGlobals(): DesktopGlobals {
  return typeof globalThis === 'undefined'
    ? {}
    : (globalThis as unknown as DesktopGlobals);
}

export function getDesktopBackendMode(): DesktopBackendMode {
  return desktopGlobals().__RUST_SREC_BACKEND_MODE__ === 'remote'
    ? 'remote'
    : 'local';
}

export function getDesktopRemoteUrl(): string | null {
  const url = desktopGlobals().__RUST_SREC_REMOTE_URL__;
  const trimmed = typeof url === 'string' ? url.trim() : '';
  return trimmed.length > 0 ? trimmed : null;
}

export function isBrowserRuntime(): boolean {
  return (
    typeof window !== 'undefined' && typeof window.localStorage !== 'undefined'
  );
}

/**
 * Identifies the active desktop backend so stored browser sessions never leak
 * across Local <-> Remote switches or between different remote servers.
 * Local mode intentionally uses `'local'` rather than the ephemeral loopback port.
 */
export function getCurrentDesktopSessionScope(): string {
  if (getDesktopBackendMode() === 'remote') {
    const remoteUrl = getDesktopRemoteUrl();
    return remoteUrl ? `remote:${remoteUrl}` : 'remote';
  }
  return 'local';
}

export function parseStoredSession(
  raw: string | null,
  expectedScope?: string,
): Partial<SessionData> {
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
    const candidate = parsed as Partial<SessionData>;
    if (expectedScope !== undefined) {
      if (candidate.backendScope !== undefined) {
        if (candidate.backendScope !== expectedScope) {
          return {};
        }
      } else if (expectedScope !== 'local') {
        // Pre-scope desktop sessions belong to the legacy local backend and
        // must never be forwarded to a remote server.
        return {};
      }
    }
    return candidate;
  } catch {
    return {};
  }
}
