import {
  errorBody,
  errorDetails,
  hasErrorCode,
  referencesFromConflict,
} from '@/lib/api-error';
import {
  ProxyReferencesSchema,
  type ProxyReferences,
} from '@/api/schemas/proxies';

export type ProxyConflict =
  /** Another proxy has the name. */
  | { kind: 'name_taken'; name: string }
  /** Another proxy, `name`, already reaches the same address with the same username. */
  | { kind: 'duplicate'; name: string }
  /** The proxy changed since it was read. */
  | { kind: 'stale' };

/** Why the backend refused to save a proxy, when it was a conflict. */
export function proxyConflict(error: unknown): ProxyConflict | undefined {
  const body = errorBody(error);
  const name = errorDetails(error)?.name;
  if (hasErrorCode(body, 'PROXY_NAME_TAKEN') && typeof name === 'string')
    return { kind: 'name_taken', name };
  if (hasErrorCode(body, 'PROXY_DUPLICATE') && typeof name === 'string')
    return { kind: 'duplicate', name };
  if (hasErrorCode(body, 'PROXY_STALE_VERSION')) return { kind: 'stale' };
  return undefined;
}

/** What still uses a proxy whose deletion the backend refused. */
export function proxyReferencesFromConflict(
  error: unknown,
): ProxyReferences | undefined {
  return referencesFromConflict(
    error,
    'PROXY_REFERENCED',
    ProxyReferencesSchema,
  );
}

/** How many routes a set of references holds. */
export function referenceCount(references: ProxyReferences): number {
  return (
    (references.global ? 1 : 0) +
    references.platforms.length +
    references.templates.length +
    references.streamers.length +
    references.accounts.length
  );
}
