import { useQuery } from '@tanstack/react-query';
import { listEvents } from '@/server/functions/notifications';
import { getLastSeenCriticalMs } from '@/lib/notification-state';

/**
 * Hook to determine if we should show a notification dot for critical events.
 * Returns true if there are critical events in the last 24h that haven't been seen.
 */
export function useNotificationDot() {
  const { data: hasCriticalDot, isLoading } = useQuery({
    queryKey: ['notification-critical-dot'],
    queryFn: async () => {
      if (typeof window === 'undefined') return false;

      const events = await listEvents({ data: { limit: 50 } });
      const cutoff = Date.now() - 24 * 60 * 60 * 1000;
      const lastSeen = getLastSeenCriticalMs();

      return events.some((e) => {
        const p = (e.priority ?? '').toString().trim().toLowerCase();
        if (p !== 'critical') return false;

        const ts = Date.parse(e.created_at);
        if (!Number.isFinite(ts)) return true;
        return ts >= cutoff && ts > lastSeen;
      });
    },
    refetchInterval: 60_000,
    staleTime: 30_000,
    retry: false,
  });

  return {
    hasCriticalDot: !!hasCriticalDot,
    isLoading,
  };
}
