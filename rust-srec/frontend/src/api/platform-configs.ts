import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import {
  getBigoNotificationStatus,
  getPlatformConfig,
  listPlatformConfigs,
} from '@/server/functions';

/** Every platform's configuration, shared by the pages that list or pick one. */
export const platformConfigsQueryOptions = queryOptions({
  queryKey: queryKeys.platformConfigs.list(),
  queryFn: () => listPlatformConfigs(),
});

export function platformConfigQueryOptions(id: string) {
  return queryOptions({
    queryKey: queryKeys.platformConfigs.detail(id),
    queryFn: () => getPlatformConfig({ data: id }),
  });
}

/**
 * Bigo's live-notification listener and each Bigo streamer's monitoring method. One entry shared
 * by every card and form; refetched when the live connection reports a change.
 */
export const bigoNotificationStatusQueryOptions = queryOptions({
  queryKey: queryKeys.platformConfigs.bigoNotifications(),
  queryFn: () => getBigoNotificationStatus(),
  staleTime: 30_000,
});

/** Refetches the Bigo live-notification status. */
export function invalidateBigoNotificationStatus(client: QueryClient) {
  return client.invalidateQueries({
    queryKey: queryKeys.platformConfigs.bigoNotifications(),
  });
}
