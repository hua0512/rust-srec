import { queryOptions, type QueryClient } from '@tanstack/react-query';
import {
  getEffectiveRoute,
  getProxy,
  getSystemProxy,
  listProxies,
} from '@/server/functions/proxies';
import type { EffectiveRouteQuery } from '@/api/schemas/proxies';

/**
 * Query keys for saved proxies. Every key hangs off `all`, so one invalidation
 * after a proxy or route change refreshes the list, details and effective
 * routes together.
 */
const proxyQueryKeys = {
  all: ['proxies'] as const,
  list: () => [...proxyQueryKeys.all, 'list'] as const,
  detail: (id: string) => [...proxyQueryKeys.all, 'detail', id] as const,
  system: () => [...proxyQueryKeys.all, 'system'] as const,
  effective: (query: EffectiveRouteQuery) =>
    [
      ...proxyQueryKeys.all,
      'effective',
      query.scope_type,
      query.scope_id ?? '',
      query.platform_id ?? '',
    ] as const,
};

export const proxiesQueryOptions = queryOptions({
  queryKey: proxyQueryKeys.list(),
  queryFn: () => listProxies(),
});

export function proxyDetailQueryOptions(id: string) {
  return queryOptions({
    queryKey: proxyQueryKeys.detail(id),
    queryFn: () => getProxy({ data: id }),
  });
}

/** Read once by the server at startup, so it never changes while it runs. */
export const systemProxyQueryOptions = queryOptions({
  queryKey: proxyQueryKeys.system(),
  queryFn: () => getSystemProxy(),
  staleTime: Infinity,
});

export function effectiveRouteQueryOptions(query: EffectiveRouteQuery) {
  return queryOptions({
    queryKey: proxyQueryKeys.effective(query),
    queryFn: () => getEffectiveRoute({ data: query }),
  });
}

/** Refreshes everything about saved proxies after one or a route changes. */
export function invalidateProxyQueries(client: QueryClient) {
  return client.invalidateQueries({ queryKey: proxyQueryKeys.all });
}
