export type SessionData = {
  username: string;
  token: {
    access_token: string;
    refresh_token: string;
    // Stored as absolute timestamps (ms since epoch)
    expires_in: number;
    refresh_expires_in: number;
  };
  roles: string[];
  mustChangePassword: boolean;
};

import { isDesktopBuild } from '@/utils/desktop';
import {
  BROWSER_SESSION_STORAGE_KEY,
  isBrowserRuntime,
  parseStoredSession,
} from './session-storage';

export function getDesktopAccessToken(): string | null {
  if (!isDesktopBuild()) return null;
  if (!isBrowserRuntime()) return null;

  const stored = parseStoredSession(
    window.localStorage.getItem(BROWSER_SESSION_STORAGE_KEY),
  );
  const token = stored.token?.access_token;
  return typeof token === 'string' && token.length > 0 ? token : null;
}

// Type guard to check if session data is complete
export function isValidSession(
  data: Partial<SessionData>,
): data is SessionData {
  return !!(
    data.username &&
    data.token?.access_token &&
    data.token?.refresh_token &&
    Array.isArray(data.roles)
  );
}

// Client-visible shape that omits the refresh token to avoid exposing it to the browser
export type ClientSessionData = Omit<SessionData, 'token'> & {
  token: {
    access_token: string;
    expires_in: number;
    refresh_expires_in: number;
  };
};

// Strip refresh token before returning session data to the client
export function sanitizeClientSession(data: SessionData): ClientSessionData {
  return {
    username: data.username,
    token: {
      access_token: data.token.access_token,
      expires_in: data.token.expires_in,
      refresh_expires_in: data.token.refresh_expires_in,
    },
    roles: data.roles,
    mustChangePassword: data.mustChangePassword ?? false,
  };
}
