import { queryOptions, type QueryClient } from '@tanstack/react-query';
import {
  getCredentialCapabilities,
  getEffectiveCredentialSelection,
  listCredentialAttention,
  listCredentialProfiles,
} from '@/server/functions/credential-profiles';
import type { CredentialOwner } from '@/api/schemas/credential-profiles';

function credentialOwnerId(owner: CredentialOwner) {
  return owner.type === 'platform'
    ? owner.platform_id
    : owner.type === 'template'
      ? owner.template_id
      : owner.streamer_id;
}

/**
 * Query keys for account profiles and the selections that use them.
 *
 * Everything about one platform hangs off `platform(id)`, so invalidating that
 * prefix refreshes its account list and every scope's effective selection on
 * it together.
 */
const credentialQueryKeys = {
  all: ['credential-profiles'] as const,
  platform: (platformId: string) =>
    [...credentialQueryKeys.all, platformId] as const,
  accounts: (platformId: string) =>
    [...credentialQueryKeys.platform(platformId), 'accounts'] as const,
  capabilities: (platformId: string) =>
    [...credentialQueryKeys.all, 'capabilities', platformId] as const,
  attention: () => [...credentialQueryKeys.all, 'attention'] as const,
  effectiveSelection: (scope: CredentialOwner, platformId: string) =>
    [
      ...credentialQueryKeys.platform(platformId),
      'selection',
      scope.type,
      credentialOwnerId(scope),
    ] as const,
};

/**
 * A platform's accounts. Templates and streamers only select them, so every
 * scope on the platform shares this one entry.
 */
export function platformAccountsQueryOptions(platformId: string) {
  return queryOptions({
    queryKey: credentialQueryKeys.accounts(platformId),
    queryFn: () =>
      listCredentialProfiles({ data: { platform_id: platformId } }),
  });
}

/**
 * What the platform's accounts accept and support. Fixed by the server's
 * providers, so it never goes stale.
 */
export function platformCapabilitiesQueryOptions(platformId: string) {
  return queryOptions({
    queryKey: credentialQueryKeys.capabilities(platformId),
    queryFn: () =>
      getCredentialCapabilities({ data: { platform_id: platformId } }),
    staleTime: Infinity,
  });
}

/**
 * Enabled accounts on every platform that need a new login or keep failing to
 * refresh. Refetched when the live connection reports a change.
 */
export const credentialAttentionQueryOptions = queryOptions({
  queryKey: credentialQueryKeys.attention(),
  queryFn: () => listCredentialAttention(),
  staleTime: 30_000,
});

/** Refetches the accounts needing attention. */
export function invalidateCredentialAttention(client: QueryClient) {
  return client.invalidateQueries({
    queryKey: credentialQueryKeys.attention(),
  });
}

/** What the saved configuration of `scope` selects on the platform. */
export function effectiveSelectionQueryOptions(
  scope: CredentialOwner,
  platformId: string,
) {
  return queryOptions({
    queryKey: credentialQueryKeys.effectiveSelection(scope, platformId),
    queryFn: () =>
      getEffectiveCredentialSelection({
        data: {
          scope_type: scope.type,
          scope_id: credentialOwnerId(scope),
          platform_id: platformId,
        },
      }),
  });
}

/**
 * Refreshes a platform's accounts and effective selections after an account
 * or a selection on it changes. Without a platform, every platform's entries
 * are refreshed, for writes that can touch several (a template's overrides, or
 * a streamer whose URL moved it to another platform).
 */
export function invalidateCredentialQueries(
  client: QueryClient,
  platformId?: string,
) {
  if (!platformId) {
    return client.invalidateQueries({ queryKey: credentialQueryKeys.all });
  }
  // The attention list spans every platform, so it is not under the
  // platform's prefix.
  return Promise.all([
    client.invalidateQueries({
      queryKey: credentialQueryKeys.platform(platformId),
    }),
    invalidateCredentialAttention(client),
  ]).then(() => undefined);
}
