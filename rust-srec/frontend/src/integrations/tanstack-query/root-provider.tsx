import {
  MutationCache,
  QueryCache,
  QueryClient,
  QueryClientProvider,
} from '@tanstack/react-query';

import { redirectToChangePasswordOnError } from '@/lib/password-change-redirect';

/**
 * How long a result is reused before a mount, a window focus or a reconnect may
 * fetch it again. Moving between pages within this window reuses what the cache
 * already holds instead of re-issuing every query the page declares. Polling is
 * unaffected: `refetchInterval` fires regardless of staleness, and anything
 * that must be current after a write goes through `invalidateQueries`, which
 * marks entries stale whatever this value is.
 */
const DEFAULT_STALE_TIME_MS = 10_000;

export function getContext() {
  // Every useQuery/useMutation error funnels through these caches, so 403
  // PASSWORD_CHANGE_REQUIRED from any API call is intercepted here without
  // per-call handling.
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: DEFAULT_STALE_TIME_MS,
        // Polling pages stay mounted in hidden tabs and none of them show
        // anything until the tab is looked at again, so intervals stay parked
        // while hidden. This restates React Query's default because the
        // polling pages rely on it instead of each opting out.
        refetchIntervalInBackground: false,
      },
    },
    queryCache: new QueryCache({
      onError: redirectToChangePasswordOnError,
    }),
    mutationCache: new MutationCache({
      onError: redirectToChangePasswordOnError,
    }),
  });
  return {
    queryClient,
  };
}

export function Provider({
  children,
  queryClient,
}: {
  children: React.ReactNode;
  queryClient: QueryClient;
}) {
  return (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}
