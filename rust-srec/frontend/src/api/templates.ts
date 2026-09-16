import { queryOptions } from '@tanstack/react-query';
import { listTemplates } from '@/server/functions';

/**
 * Templates only change through the templates pages, and every one of those
 * writes invalidates this key, so moving between the pages that offer a
 * template picker can reuse a single fetch for this long.
 */
const TEMPLATES_STALE_TIME_MS = 60_000;

/**
 * The template list, shared by every page that reads it so they cannot drift
 * into different caching behaviour for the same key.
 *
 * Note for callers: default the result at the destructure rather than passing
 * `initialData: []`. Seeded initial data counts as freshly fetched, which under
 * any non-zero stale time suppresses the fetch on mount and leaves the picker
 * empty until an unrelated refetch happens to run.
 */
export const templatesQueryOptions = queryOptions({
  queryKey: ['templates'],
  queryFn: () => listTemplates(),
  staleTime: TEMPLATES_STALE_TIME_MS,
});
