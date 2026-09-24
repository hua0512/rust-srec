import { createFileRoute } from '@tanstack/react-router';
import {
  pipelineStatsQueryOptions,
  systemHealthQueryOptions,
} from '@/api/dashboard';
import { awaitOnServer } from '@/lib/route-prefetch';

export const Route = createFileRoute('/_authed/_dashboard/dashboard')({
  // Only the summaries are loaded ahead. The live streamer grid is left to the
  // page: with many streamers live, rendering their cards on the server made
  // the page slower to become usable, since the browser had to parse, lay out
  // and hydrate all of them before anything responded.
  loader: ({ context: { queryClient } }) =>
    awaitOnServer(
      Promise.all([
        queryClient.prefetchQuery(systemHealthQueryOptions),
        queryClient.prefetchQuery(pipelineStatsQueryOptions),
      ]),
    ),
});
