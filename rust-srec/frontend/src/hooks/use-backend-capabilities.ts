import { useQuery } from '@tanstack/react-query';
import { systemHealthQueryOptions } from '@/api/dashboard';

const EMPTY_CAPABILITIES: readonly string[] = [];

/**
 * Reads optional backend capabilities reported by `/api/health`.
 * Defaults to empty until loaded or when connected to a backend that omits the field.
 */
export function useBackendCapabilities(): readonly string[] {
  const { data } = useQuery({
    ...systemHealthQueryOptions,
    select: (health) => health.capabilities,
    staleTime: 5 * 60 * 1000,
    retry: false,
    refetchOnWindowFocus: false,
  });
  return data ?? EMPTY_CAPABILITIES;
}

export function useHasBackendCapability(capability: string): boolean {
  return useBackendCapabilities().includes(capability);
}
