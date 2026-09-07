import { queryOptions } from '@tanstack/react-query';
import { checkAuthFn } from '@/server/functions';

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

export const sessionQueryOptions = queryOptions({
  queryKey: ['session'],
  // Only an explicit null means unauthenticated; rejected checks must leave
  // React Query's last known session intact for retry.
  queryFn: () => checkAuthFn(),
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
    const untilRenewal = expiry - Date.now() - TOKEN_RENEWAL_BUFFER_MS;
    return Math.max(0, Math.min(SESSION_MAX_STALE_TIME_MS, untilRenewal));
  },
});
