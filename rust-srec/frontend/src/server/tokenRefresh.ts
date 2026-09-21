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
import { sanitizeClientSession, isValidSession } from '../utils/session';
import { useAppSession } from '../utils/session.server';
import { BASE_URL } from '../utils/env';
import { ACCOUNT_DISABLED_CODE } from '../lib/api-error';

type RefreshOutcome = {
  accessToken: string;
  refreshToken: string;
  accessExpiry: number;
  refreshExpiry: number;
  roles?: string[];
  mustChangePassword?: boolean;
};

/**
 * Result of one refresh attempt.
 *
 * `rejected` means `/auth/refresh` answered that the presented refresh token is
 * not usable; it is the only outcome that justifies clearing the session.
 * `transient` covers every other failure — the endpoint was unreachable, timed
 * out, or failed for a reason unrelated to the token. The refresh token in the
 * session is then still whatever it was, so callers keep the session and go on
 * using the access token they already hold.
 */
type PerformRefreshResult =
  | { status: 'refreshed'; outcome: RefreshOutcome }
  | { status: 'rejected' }
  | { status: 'transient' };

export type TokenRefreshResult =
  | { status: 'refreshed'; accessToken: string }
  | { status: 'rejected' }
  | { status: 'transient' }
  // The caller's session changed while it waited. Never retry its request
  // using credentials from a different sign-in.
  | { status: 'superseded' };

const RECENT_ROTATION_TTL_MS = 60_000;
const MAX_MAP_SIZE = 1000;
const inFlightRefreshByRefreshToken = new Map<
  string,
  Promise<PerformRefreshResult>
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

function recordRotation(oldRefreshToken: string, outcome: RefreshOutcome) {
  if (outcome.refreshToken === oldRefreshToken) return;

  const now = Date.now();
  // Keep every live predecessor pointing at the latest successor. Preserve
  // its original deadline so reading an old cookie cannot extend its lifetime.
  for (const entry of recentRotationByOldRefreshToken.values()) {
    if (
      entry.expiresAt > now &&
      entry.outcome.refreshToken === oldRefreshToken
    ) {
      entry.outcome = outcome;
    }
  }
  recentRotationByOldRefreshToken.set(oldRefreshToken, {
    outcome,
    expiresAt: now + RECENT_ROTATION_TTL_MS,
  });
  cleanupRotationMap();
}

async function latestRotation(
  outcome: RefreshOutcome,
): Promise<PerformRefreshResult> {
  // A cached successor can itself be rotating. Wait for that attempt rather
  // than write a token the backend is already consuming into another cookie.
  for (;;) {
    const recent = getRecentRotation(outcome.refreshToken);
    if (recent) {
      outcome = recent;
      continue;
    }
    const pending = inFlightRefreshByRefreshToken.get(outcome.refreshToken);
    if (!pending) return { status: 'refreshed', outcome };
    const result = await pending;
    if (result.status === 'rejected') return result;
    if (result.status === 'transient') return { status: 'refreshed', outcome };
    outcome = result.outcome;
  }
}

async function applyOutcomeToSession({
  session,
  currentSessionData,
  outcome,
}: {
  session: Awaited<ReturnType<typeof useAppSession>>;
  currentSessionData: SessionData;
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

  await session.update(userData);
}

/**
 * Attempt to refresh the authentication token.
 *
 * This function ensures that only one refresh request is in flight at any time,
 * preventing race conditions that could occur when:
 * 1. Multiple API calls fail with 401 simultaneously
 * 2. checkAuthFn runs concurrently with API calls
 *
 * The session is cleared only for a `rejected` result. A `transient` result
 * leaves the session — including its never-consumed refresh token — in place so
 * a later call can retry.
 */
export async function refreshAuthTokenGlobal(): Promise<TokenRefreshResult> {
  const session = await useAppSession();
  const currentData = session.data;
  if (!isValidSession(currentData)) {
    await session.clear();
    return { status: 'rejected' };
  }
  const currentRefreshToken = currentData.token.refresh_token;

  const recent = getRecentRotation(currentRefreshToken);
  let result: PerformRefreshResult;
  if (recent) {
    result = { status: 'refreshed', outcome: recent };
  } else {
    let refreshPromise = inFlightRefreshByRefreshToken.get(currentRefreshToken);
    if (!refreshPromise) {
      refreshPromise = performRefresh({
        refreshToken: currentRefreshToken,
        fallbackAccessExpiry: currentData.token.expires_in,
        fallbackRefreshExpiry: currentData.token.refresh_expires_in,
      })
        .then((result) => {
          // Publish before releasing the in-flight entry or awaiting any caller's
          // session write. A failed cookie write must not lose a consumed token.
          if (result.status === 'refreshed') {
            recordRotation(currentRefreshToken, result.outcome);
          }
          return result;
        })
        .finally(() => {
          inFlightRefreshByRefreshToken.delete(currentRefreshToken);
        });
      inFlightRefreshByRefreshToken.set(currentRefreshToken, refreshPromise);
    }
    result = await refreshPromise;
  }

  if (result.status === 'refreshed') {
    result = await latestRotation(result.outcome);
  }

  // Desktop shares one mutable session across calls. The refresh token is a
  // sign-in/rotation identity: a logout or new login replaces it synchronously.
  // Independent web requests still persist their own copy of a shared outcome.
  if (
    session.data.username !== currentData.username ||
    session.data.token?.refresh_token !== currentRefreshToken
  ) {
    if (
      result.status === 'refreshed' &&
      session.data.username === currentData.username &&
      session.data.token?.refresh_token === result.outcome.refreshToken &&
      session.data.token.access_token === result.outcome.accessToken
    ) {
      // Another waiter already applied this rotation to the shared session.
      return { status: 'refreshed', accessToken: result.outcome.accessToken };
    }
    return { status: 'superseded' };
  }

  if (result.status === 'rejected') {
    await session.clear();
    return { status: 'rejected' };
  }

  if (result.status === 'transient') {
    return { status: 'transient' };
  }

  await applyOutcomeToSession({
    session,
    currentSessionData: currentData,
    outcome: result.outcome,
  });

  if (
    session.data.username !== currentData.username ||
    session.data.token?.refresh_token !== result.outcome.refreshToken
  ) {
    return { status: 'superseded' };
  }
  return { status: 'refreshed', accessToken: result.outcome.accessToken };
}

/**
 * Whether `/auth/refresh` answered in a way that makes re-presenting the same
 * refresh token pointless. This is the only justification for clearing the
 * session, so each answer is classified deliberately:
 *
 * - 401 — the token is unknown, already revoked, or past its expiry.
 * - 403 carrying ACCOUNT_DISABLED_CODE — the account behind the token was
 *   deactivated. Keeping the session here would strand the user in an app
 *   where every call fails and `/login` bounces them back to `/dashboard`.
 *   A 403 without that code came from something in front of the backend
 *   (a WAF, an nginx `deny`) and says nothing about the token, so it stays
 *   transient rather than signing the user out for good.
 * - 400 / 422 — rejected by the request-body extractor, before the token was
 *   looked up or rotated, so the identical body will be rejected again.
 *
 * Everything else stays transient: 503 while the auth service is unavailable,
 * any 5xx, a 404 from a misrouted reverse proxy, a 429 from one in front of
 * the backend.
 */
function isDefinitiveRejection(status: number, code?: string): boolean {
  if (status === 400 || status === 401 || status === 422) return true;
  return status === 403 && code === ACCOUNT_DISABLED_CODE;
}

function firstString(
  record: Record<string, unknown>,
  keys: string[],
): string | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === 'string' && value) return value;
  }
  return undefined;
}

/**
 * Best-effort read of the backend's error envelope. `code` decides the
 * classification in isDefinitiveRejection; `detail` is only logged.
 */
async function readErrorBody(
  response: Response,
): Promise<{ code?: string; detail?: string }> {
  try {
    const errorText = await response.text();
    if (!errorText) return {};
    try {
      const parsed = JSON.parse(errorText);
      if (parsed && typeof parsed === 'object') {
        const record = parsed as Record<string, unknown>;
        return {
          code: firstString(record, ['code']),
          detail:
            firstString(record, ['message', 'detail', 'error']) ??
            JSON.stringify(parsed),
        };
      }
      return { detail: String(parsed) };
    } catch {
      return { detail: errorText };
    }
  } catch {
    return {};
  }
}

/**
 * Resolves a failure that carries no status to classify — a transport error,
 * or a 200 whose body could not be used — against
 * recentRotationByOldRefreshToken: a concurrent call may have rotated this
 * token in the meantime, and that outcome stands. The `!response.ok` path does
 * the same lookup inline so it can rank a cached rotation above its own status
 * classification.
 */
function rotatedOrTransient(refreshToken: string): PerformRefreshResult {
  const rotated = getRecentRotation(refreshToken);
  return rotated
    ? { status: 'refreshed', outcome: rotated }
    : { status: 'transient' };
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
}): Promise<PerformRefreshResult> {
  const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
  const url = `${baseUrl}/auth/refresh`;

  let response: Response;
  try {
    // console.log(`[TokenRefresh] POST ${url} with token: ${refreshToken.slice(0, 10)}...`);
    // No client-side abort: `/auth/refresh` revokes the old refresh token
    // before it answers, so aborting a slow response would leave this session
    // holding a token the backend has already revoked and the next attempt
    // would read the resulting 401 as a definitive rejection. The request is
    // bounded by undici's header timeout instead.
    response = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });
  } catch (error) {
    // fetch rejects for transport-level problems (DNS failure, refused
    // connection, socket timeout); none of them carry an answer about the
    // token, so the session is kept.
    console.error('[TokenRefresh] Refresh endpoint unreachable:', error);
    return rotatedOrTransient(refreshToken);
  }

  if (!response.ok) {
    // A concurrent rotation outranks the status: our own request lost the race
    // and its 401 describes the token the other call already replaced.
    const rotated = getRecentRotation(refreshToken);
    if (rotated) return { status: 'refreshed', outcome: rotated };

    const { code, detail } = await readErrorBody(response);
    const wwwAuthenticate =
      response.headers.get('www-authenticate') ?? undefined;
    console.error(
      `[TokenRefresh] Refresh failed: ${response.status}${detail ? ` (${detail})` : ''}${wwwAuthenticate ? ` [www-authenticate: ${wwwAuthenticate}]` : ''}`,
    );
    return {
      status: isDefinitiveRejection(response.status, code)
        ? 'rejected'
        : 'transient',
    };
  }

  let json: any;
  try {
    json = await response.json();
  } catch (error) {
    console.error('[TokenRefresh] Refresh response was not JSON:', error);
    return rotatedOrTransient(refreshToken);
  }

  // Guard applyOutcomeToSession: a 200 without a usable access token would
  // otherwise be written into the session and break every later request.
  if (typeof json?.access_token !== 'string' || !json.access_token) {
    console.error('[TokenRefresh] Refresh response carried no access token.');
    return rotatedOrTransient(refreshToken);
  }

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
    status: 'refreshed',
    outcome: {
      accessToken: json.access_token,
      refreshToken: json.refresh_token || refreshToken,
      accessExpiry: computedAccessExpiry,
      refreshExpiry: computedRefreshExpiry,
      roles: json.roles,
      mustChangePassword: json.must_change_password,
    },
  };
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
    const result = await refreshAuthTokenGlobal();

    if (result.status === 'rejected') {
      return null;
    }

    if (result.status === 'transient') {
      // refreshAuthTokenGlobal left the session untouched, so the caller stays
      // signed in with the access token already in it; fetchBackend refreshes
      // again on the next 401 once the backend answers.
      return isValidSession(session.data)
        ? sanitizeClientSession(session.data)
        : null;
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
