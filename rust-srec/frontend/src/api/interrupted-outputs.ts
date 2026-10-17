import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { listInterruptedOutputs } from '@/server/functions/pipeline';

/** Every listing hangs off `all`, so one invalidation refreshes them together. */
const interruptedOutputKeys = {
  all: ['pipeline', 'interrupted-outputs'] as const,
  list: (jobId?: string) =>
    [...interruptedOutputKeys.all, jobId ?? 'all'] as const,
};

/**
 * Partial files interrupted jobs left behind. The list only changes when the
 * server restarts or the user settles an entry, so it is polled rarely.
 */
export function interruptedOutputsQueryOptions(jobId?: string) {
  return queryOptions({
    queryKey: interruptedOutputKeys.list(jobId),
    queryFn: () => listInterruptedOutputs({ data: { jobId } }),
    staleTime: 60_000,
    refetchInterval: 5 * 60_000,
  });
}

export function invalidateInterruptedOutputs(client: QueryClient) {
  return client.invalidateQueries({ queryKey: interruptedOutputKeys.all });
}
