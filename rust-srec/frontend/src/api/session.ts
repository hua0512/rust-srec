import { queryOptions } from '@tanstack/react-query';
import { checkAuthFn } from '@/server/functions';

export const sessionQueryOptions = queryOptions({
  queryKey: ['session'],
  // Only an explicit null means unauthenticated; rejected checks must leave
  // React Query's last known session intact for retry.
  queryFn: () => checkAuthFn(),
});
