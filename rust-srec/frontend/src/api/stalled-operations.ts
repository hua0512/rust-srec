import { queryOptions } from '@tanstack/react-query';
import { listStalledOperations } from '@/server/functions/pipeline';

/**
 * Ended jobs whose file operation still holds a worker. The list lives in
 * backend memory only, so polling it is cheap; it changes when a disk or share
 * stops or resumes answering, which no live event reports.
 */
export const stalledOperationsQueryOptions = queryOptions({
  queryKey: ['pipeline', 'stalled-operations'],
  queryFn: () => listStalledOperations(),
  refetchInterval: 15_000,
  staleTime: 10_000,
});
