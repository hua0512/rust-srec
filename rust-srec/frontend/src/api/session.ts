import { queryOptions } from '@tanstack/react-query';
import { checkAuthFn } from '@/server/functions/auth';

/**
 * Longest a session check is reused. Short enough that a session revoked on
 * another device is noticed promptly, long enough that a burst of navigations
 * costs one check rather than one per route change.
 */
const SESSION_MAX_STALE_TIME_MS = 30_000;

/**
 * Matches the window in which `ensureValidToken` renews the access token. A
 * cached session inside that window is treated as stale so the next check
 * performs the renewal instead of handing out a token about to expire.
 */
const TOKEN_RENEWAL_BUFFER_MS = 30_000;

/** Longest wait between background session checks. */
const SESSION_POLL_MAX_MS = 60_000;

/**
 * How far into the renewal window a scheduled check lands, so that a timer
 * firing slightly early still finds the token due for renewal.
 */
const SESSION_POLL_RENEWAL_LEAD_MS = 5_000;

/** Shortest wait between checks. */
const SESSION_POLL_MIN_MS = 5_000;

/**
 * When the session should next be checked so the access token is renewed
 * before it expires. Sockets are closed by the backend once the token they
 * were opened with expires, so waiting a fixed interval could let it lapse
 * and leave a reopened socket with nothing valid to present.
 *
 * Past the renewal point the token is overdue: renewal failed for now (the
 * backend is unreachable) or this clock runs ahead of the server's. Each
 * check then waits as long as the token has already been overdue, which
 * doubles the wait every check up to the longest one instead of asking
 * for a renewal every few seconds in every tab.
 */
export function sessionPollInterval(
  session: { token: { expires_in: number } } | null | undefined,
  now = Date.now(),
): number {
  const expiry = session?.token.expires_in;
  if (expiry === undefined || !Number.isFinite(expiry)) {
    return SESSION_POLL_MAX_MS;
  }
  const renewAt =
    expiry - TOKEN_RENEWAL_BUFFER_MS + SESSION_POLL_RENEWAL_LEAD_MS;
  return Math.max(
    SESSION_POLL_MIN_MS,
    Math.min(SESSION_POLL_MAX_MS, Math.abs(renewAt - now)),
  );
}

export const sessionQueryOptions = queryOptions({
  queryKey: ['session'],
  // Only an explicit null means unauthenticated; rejected checks must leave
  // React Query's last known session intact for retry.
  queryFn: () => checkAuthFn(),
  // The guard awaits this before it can redirect, so a failing endpoint must
  // fail fast rather than hold navigation open for a retry sequence.
  retry: false,
  // The `/_authed` guard reads this through `fetchQuery`, so this decides how
  // often navigating re-verifies the session. `0` for an absent or
  // unauthenticated result keeps the guard honest: signing in must be visible
  // to the very next navigation, and nothing may pass on a remembered `null`.
  staleTime: (query) => {
    const session = query.state.data;
    if (!session) return 0;
    const expiry = session.token.expires_in;
    // No usable expiry means no basis for trusting the result; check again.
    if (!Number.isFinite(expiry)) return 0;
    // Staleness is measured from when the result was written, not from now,
    // so the window has to be expressed against that same instant.
    const untilRenewal =
      expiry - TOKEN_RENEWAL_BUFFER_MS - query.state.dataUpdatedAt;
    return Math.max(0, Math.min(SESSION_MAX_STALE_TIME_MS, untilRenewal));
  },
});
