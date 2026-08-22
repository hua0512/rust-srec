import {
  MutationCache,
  QueryCache,
  QueryClient,
  QueryClientProvider,
} from '@tanstack/react-query';

import { redirectToChangePasswordOnError } from '@/lib/password-change-redirect';

export function getContext() {
  // Every useQuery/useMutation error funnels through these caches, so 403
  // PASSWORD_CHANGE_REQUIRED from any API call is intercepted here without
  // per-call handling.
  const queryClient = new QueryClient({
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
