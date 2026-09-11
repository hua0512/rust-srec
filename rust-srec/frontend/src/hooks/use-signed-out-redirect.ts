import { useEffect } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useRouter, useRouterState } from '@tanstack/react-router';

import { sessionQueryOptions } from '@/api/session';

/**
 * Sends a visitor whose session has ended from an authenticated page to the
 * login page.
 *
 * The `/_authed` guard only runs when the router navigates, so a sign-in that
 * expires while a page is open — the periodic session check answers `null` —
 * would otherwise leave the page in place with every request failing until the
 * visitor navigates or reloads by hand. This hook watches the same session
 * query and performs the guard's redirect, including the `redirect` search
 * param that brings the visitor back to this page after signing in.
 *
 * The query is observed, not fetched: `enabled: false` leaves the checks to
 * the provider's poll and to the guard, so mounting this adds no request. Only
 * an explicit `null` is a signed-out answer; `undefined` means no check has
 * run yet and is left alone.
 *
 * A navigation in flight is left to run its course. Its guard re-checks the
 * session and redirects on its own, and the sign-out and password-change flows
 * write `null` right before navigating away themselves, so acting during that
 * window would only pile a second navigation on top of theirs. Should the
 * navigation commit with the session still signed out, the effect runs again
 * and redirects then.
 */
export function useSignedOutRedirect(): void {
  const router = useRouter();
  const { data: session } = useQuery({
    ...sessionQueryOptions,
    enabled: false,
  });
  const isNavigating = useRouterState({
    select: (state) => state.status === 'pending',
  });
  const href = useRouterState({ select: (state) => state.location.href });

  useEffect(() => {
    if (session !== null) return;
    if (isNavigating) return;
    void router.navigate({
      to: '/login',
      search: { redirect: href },
      replace: true,
    });
  }, [session, isNavigating, href, router]);
}
