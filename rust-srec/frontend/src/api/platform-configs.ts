import { queryOptions } from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import { getPlatformConfig, listPlatformConfigs } from '@/server/functions';

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
