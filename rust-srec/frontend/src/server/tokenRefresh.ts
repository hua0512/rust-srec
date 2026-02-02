/**
 * Global token refresh coordination module.
 *
 * This module provides a centralized mechanism for refreshing authentication tokens
 * to prevent race conditions when multiple concurrent requests detect an expired token.
 *
 * The key insight is that token rotation on the backend immediately revokes the old
 * refresh token, so we must ensure only ONE refresh attempt happens at a time globally.
 */

import type { ClientSessionData, SessionData } from '../utils/session';
import {
  sanitizeClientSession,
  isValidSession,
  useAppSession,
} from '../utils/session';
import { BASE_URL } from '../utils/env';

type RefreshOutcome = {
  accessToken: string;
  refreshToken: string;
  accessExpiry: number;
  refreshExpiry: number;
  roles?: string[];
  mustChangePassword?: boolean;
};

const RECENT_ROTATION_TTL_MS = 60_000;
const MAX_MAP_SIZE = 1000;
const inFlightRefreshByRefreshToken = new Map<
  string,
  Promise<RefreshOutcome | null>
>();
const recentRotationByOldRefreshToken = new Map<
  string,
  { outcome: RefreshOutcome; expiresAt: number }
>();

/**
 * Periodically clean up the rotation map to prevent memory leaks on long-running servers.
 */
function cleanupRotationMap() {
  if (recentRotationByOldRefreshToken.size > MAX_MAP_SIZE) {
    const now = Date.now();
    for (const [key, value] of recentRotationByOldRefreshToken.entries()) {
      if (now > value.expiresAt) {
        recentRotationByOldRefreshToken.delete(key);
      }
    }
  }

  // If still too large, clear oldest (approximate via iterator)
  if (recentRotationByOldRefreshToken.size > MAX_MAP_SIZE) {
    const keysToDelete = Array.from(
      recentRotationByOldRefreshToken.keys(),
    ).slice(0, recentRotationByOldRefreshToken.size - MAX_MAP_SIZE);
    for (const key of keysToDelete) {
      recentRotationByOldRefreshToken.delete(key);
    }
  }
}

function getRecentRotation(refreshToken: string): RefreshOutcome | null {
  const entry = recentRotationByOldRefreshToken.get(refreshToken);
  if (!entry) return null;
  if (Date.now() > entry.expiresAt) {
    recentRotationByOldRefreshToken.delete(refreshToken);
    return null;
  }
  return entry.outcome;
}

async function applyOutcomeToSession({
  session,
  currentSessionData,
  oldRefreshToken,
  outcome,
}: {
  session: any;
  currentSessionData: SessionData;
  oldRefreshToken: string;
  outcome: RefreshOutcome;
}) {
  const userData: SessionData = {
    username: currentSessionData.username,
    token: {
      access_token: outcome.accessToken,
      refresh_token: outcome.refreshToken,
      expires_in: outcome.accessExpiry,
      refresh_expires_in: outcome.refreshExpiry,
    },
    roles: outcome.roles ?? currentSessionData.roles,
    mustChangePassword:
      outcome.mustChangePassword ?? currentSessionData.mustChangePassword,
  };

  // console.log(
  //   `[TokenRefresh] Applying new tokens to session. Access: ${outcome.accessToken.slice(0, 10)}..., Refresh: ${outcome.refreshToken.slice(0, 10)}...`,
  // );
  await session.update(userData);

  if (outcome.refreshToken !== oldRefreshToken) {
    cleanupRotationMap();
    recentRotationByOldRefreshToken.set(oldRefreshToken, {
      outcome,
      expiresAt: Date.now() + RECENT_ROTATION_TTL_MS,
    });
  }
}

/**
 * Attempt to refresh the authentication token.
 *
 * This function ensures that only one refresh request is in flight at any time,
 * preventing race conditions that could occur when:
 * 1. Multiple API calls fail with 401 simultaneously
 * 2. checkAuthFn runs concurrently with API calls
 *
 * @returns The new access token if successful, null if refresh failed
 */
export async function refreshAuthTokenGlobal(): Promise<string | null> {
  const session = await useAppSession();
  const currentData = session.data;
  if (!isValidSession(currentData)) {
    await session.clear();
    return null;
  }
  const currentRefreshToken = currentData.token?.refresh_token;

  if (!currentRefreshToken) {
    // console.log('[TokenRefresh] No refresh token available in session.');
    return null;
  }

  const recent = getRecentRotation(currentRefreshToken);
  if (recent) {
    // console.log(
    //   `[TokenRefresh] Refresh token was recently rotated (outcome cached). Using new access token: ${recent.accessToken.slice(0, 10)}...`,
    // );
    await applyOutcomeToSession({
      session,
      currentSessionData: currentData,
      oldRefreshToken: currentRefreshToken,
      outcome: recent,
    });
    return recent.accessToken;
  }

  // If a refresh is already in progress for this refresh token, wait for it
  let refreshPromise = inFlightRefreshByRefreshToken.get(currentRefreshToken);
  if (refreshPromise) {
    // console.log(
    //   `[TokenRefresh] Refresh already in progress for token ${currentRefreshToken.slice(0, 10)}..., waiting...`,
    // );
  } else {
    refreshPromise = performRefresh({
      refreshToken: currentRefreshToken,
      fallbackAccessExpiry: currentData.token?.expires_in,
      fallbackRefreshExpiry: currentData.token?.refresh_expires_in,
    });
    inFlightRefreshByRefreshToken.set(currentRefreshToken, refreshPromise);
    void refreshPromise.finally(() => {
      inFlightRefreshByRefreshToken.delete(currentRefreshToken);
    });
  }

  const outcome = await refreshPromise;

  if (!outcome) {
    // console.log('[TokenRefresh] Refresh failed (no outcome), clearing session.');
    await session.clear();
    return null;
  }

  await applyOutcomeToSession({
    session,
    currentSessionData: currentData,
    oldRefreshToken: currentRefreshToken,
    outcome,
  });

  return outcome.accessToken;
}

/**
 * Perform the actual token refresh.
 */
async function performRefresh({
  refreshToken,
  fallbackAccessExpiry,
  fallbackRefreshExpiry,
}: {
  refreshToken: string;
  fallbackAccessExpiry?: number;
  fallbackRefreshExpiry?: number;
}): Promise<RefreshOutcome | null> {
  try {
    // console.log('[TokenRefresh] Calling refresh endpoint...');

    const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
    const url = `${baseUrl}/auth/refresh`;
    // console.log(`[TokenRefresh] POST ${url} with token: ${refreshToken.slice(0, 10)}...`);
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });

    if (!response.ok) {
      const rotated = getRecentRotation(refreshToken);
      if (rotated) {
        // console.log(
        //   '[TokenRefresh] Token was rotated by another request, using new token.',
        // );
        return rotated;
      }

      let detail: string | undefined;
      try {
        const errorText = await response.text();
        if (errorText) {
          try {
            const parsed = JSON.parse(errorText);
            if (parsed && typeof parsed === 'object') {
              detail =
                (parsed as any).message ||
                (parsed as any).detail ||
                (parsed as any).error ||
                JSON.stringify(parsed);
            } else {
              detail = String(parsed);
            }
          } catch {
            detail = errorText;
          }
        }
      } catch {
        // ignore body parsing errors
      }

      const wwwAuthenticate =
        response.headers.get('www-authenticate') ?? undefined;
      // console.error(
      //   `[TokenRefresh] Refresh failed with status: ${response.status}${detail ? ` (${detail})` : ''}${wwwAuthenticate ? ` [www-authenticate: ${wwwAuthenticate}]` : ''}`,
      // );
      throw new Error(
        `Refresh failed: ${response.status}${detail ? ` (${detail})` : ''}${wwwAuthenticate ? ` [www-authenticate: ${wwwAuthenticate}]` : ''}`,
      );
    }

    const json = await response.json();
    const now = Date.now();

    const computedAccessExpiry =
      typeof json.expires_in === 'number' && Number.isFinite(json.expires_in)
        ? now + json.expires_in * 1000
        : (fallbackAccessExpiry ?? now);

    const computedRefreshExpiry =
      typeof json.refresh_expires_in === 'number' &&
      Number.isFinite(json.refresh_expires_in)
        ? now + json.refresh_expires_in * 1000
        : (fallbackRefreshExpiry ?? now);

    // console.log(
    //   `[TokenRefresh] Token refreshed successfully. Access expiry: ${new Date(computedAccessExpiry).toLocaleString()}, Refresh expiry: ${new Date(computedRefreshExpiry).toLocaleString()}`,
    // );
    return {
      accessToken: json.access_token,
      refreshToken: json.refresh_token || refreshToken,
      accessExpiry: computedAccessExpiry ?? now,
      refreshExpiry: computedRefreshExpiry ?? now,
      roles: json.roles,
      mustChangePassword: json.must_change_password,
    };
  } catch (error) {
    console.error('[TokenRefresh] Failed to refresh token:', error);

    const rotated = getRecentRotation(refreshToken);
    if (rotated) {
      // console.log(
      //   '[TokenRefresh] Token was rotated by another request during error, using new token.',
      // );
      return rotated;
    }
    return null;
  }
}

/**
 * Check if a valid access token exists and is not expired.
 * If expired, attempt to refresh.
 *
 * @returns ClientSessionData if authenticated, null if not authenticated
 */
export async function ensureValidToken(): Promise<ClientSessionData | null> {
  const session = await useAppSession();
  const token = session.data.token;
  const username = session.data.username;

  // Check if we have the minimum required data
  if (!token?.refresh_token || !username) {
    // console.log(`[TokenRefresh] Missing ${!token?.refresh_token ? 'refresh_token' : ''}${!token?.refresh_token && !username ? ' and ' : ''}${!username ? 'username' : ''} in session.`);
    return null;
  }

  const now = Date.now();

  // Check if refresh token is expired
  const refreshExpiry = token.refresh_expires_in ?? 0;
  if (now >= refreshExpiry) {
    // console.log('[TokenRefresh] Refresh token expired, clearing session.');
    await session.clear();
    return null;
  }

  // Check if access token is expired or about to expire (with 30s buffer)
  const accessExpiry = token.expires_in ?? 0;
  const buffer = 30000;
  if (now >= accessExpiry - buffer) {
    // console.log(
    //   `[TokenRefresh] Access token expired or expiring soon. Now: ${new Date(now).toLocaleString()}, Expiry: ${new Date(accessExpiry).toLocaleString()}, Buffer: ${buffer}ms. Refreshing...`,
    // );
    const newAccessToken = await refreshAuthTokenGlobal();

    if (!newAccessToken) {
      return null;
    }

    // Re-read session to get updated data
    const updatedSession = await useAppSession();
    const updatedData = updatedSession.data;

    // Type guard: ensure we have all required fields
    if (!isValidSession(updatedData)) {
      return null;
    }

    return sanitizeClientSession(updatedData);
  }

  // Type guard: ensure we have all required fields
  if (!isValidSession(session.data)) {
    return null;
  }

  return sanitizeClientSession(session.data);
}
