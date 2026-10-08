import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import type { I18n, MessageDescriptor } from '@lingui/core';
import { proxiesQueryOptions } from '@/api/proxies';
import type {
  ProxyRoute,
  RouteSource,
  SavedProxy,
} from '@/api/schemas/proxies';
import { NAME_LINK } from '@/components/shared/list-panel';

/** `host:port`, or the host alone when the URL leaves the port out. */
export function proxyAddress(proxy: Pick<SavedProxy, 'host' | 'port'>) {
  return proxy.port != null ? `${proxy.host}:${proxy.port}` : proxy.host;
}

/** Whether two routes choose the same connection. */
export function sameRoute(a: ProxyRoute, b: ProxyRoute) {
  return (
    a.kind === b.kind &&
    (a.kind !== 'proxy' || (b.kind === 'proxy' && a.id === b.id))
  );
}

/** FFmpeg reads only `http://` proxies. */
export function ffmpegCanUse(proxy: Pick<SavedProxy, 'scheme'>) {
  return proxy.scheme === 'http';
}

export const DIRECT_LABEL = msg`Direct`;
export const SYSTEM_LABEL = msg`System proxy`;

/** Which setting decided a route, as "from …". */
export const ROUTE_SOURCE_LABELS: Record<RouteSource, MessageDescriptor> = {
  account: msg`from the account`,
  streamer: msg`from the streamer`,
  template: msg`from the template`,
  platform: msg`from the platform`,
  global: msg`from global settings`,
};

/** The saved proxies, for mapping a route's proxy id to its name. */
export function useSavedProxies() {
  return useQuery(proxiesQueryOptions);
}

/**
 * What a route that does not inherit connects through: Direct, System proxy,
 * or the saved proxy's name. A proxy missing from `proxies` (still loading, or
 * deleted elsewhere) reads as "Saved proxy".
 */
export function routeName(
  i18n: I18n,
  route: Exclude<ProxyRoute, { kind: 'inherit' }>,
  proxies: SavedProxy[] | undefined,
): string {
  switch (route.kind) {
    case 'direct':
      return i18n._(DIRECT_LABEL);
    case 'system':
      return i18n._(SYSTEM_LABEL);
    default:
      return (
        proxies?.find((proxy) => proxy.id === route.id)?.name ??
        i18n._(msg`Saved proxy`)
      );
  }
}

/**
 * A route as a value in a details list: a saved proxy's name links to the
 * Proxies page, and an inheriting route is muted. For an account, inheriting
 * means following the recording that uses it.
 */
export function ProxyRouteValue({
  route,
  account = false,
}: {
  route: ProxyRoute | undefined;
  account?: boolean;
}) {
  const { i18n } = useLingui();
  const proxies = useSavedProxies().data;
  if (!route || route.kind === 'inherit')
    return (
      <span className="text-muted-foreground/80">
        {account ? (
          <Trans>Follows the recording</Trans>
        ) : (
          <Trans context="proxy route">Inherited</Trans>
        )}
      </span>
    );
  const name = routeName(i18n, route, proxies);
  if (route.kind !== 'proxy') return <span>{name}</span>;
  return (
    <Link to="/config/proxies" className={NAME_LINK}>
      {name}
    </Link>
  );
}
