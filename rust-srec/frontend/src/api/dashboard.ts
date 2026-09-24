import { queryOptions } from '@tanstack/react-query';
import { getSystemHealth } from '@/server/functions/system';
import { getPipelineStats } from '@/server/functions/pipeline';
import { listStreamers } from '@/server/functions/streamers';

// The dashboard's summaries, shared by its route loader and the page so the
// page finds the entries the loader filled.

export const systemHealthQueryOptions = queryOptions({
  queryKey: ['health'],
  queryFn: () => getSystemHealth(),
});

export const pipelineStatsQueryOptions = queryOptions({
  queryKey: ['pipeline', 'stats'],
  queryFn: () => getPipelineStats(),
});

export const liveStreamersQueryOptions = queryOptions({
  queryKey: ['streamers', 'active'],
  queryFn: () => listStreamers({ data: { limit: 100, state: 'LIVE' } }),
});
