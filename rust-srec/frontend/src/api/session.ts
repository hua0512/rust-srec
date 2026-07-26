import { queryOptions } from '@tanstack/react-query';
import { checkAuthFn } from '@/server/functions';

export const sessionQueryOptions = queryOptions({
  queryKey: ['session'],
  // Let checkAuthFn errors propagate so React Query keeps the last known
  // session and applies retry/backoff on transient failures; only an
  // explicit null (unauthenticated from ensureValidToken) clears the
  // session state.
  queryFn: async () => {
    return await checkAuthFn();
  },
});
