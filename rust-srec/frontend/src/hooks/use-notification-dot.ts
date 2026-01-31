import { useQuery } from '@tanstack/react-query';
import { listEvents } from '@/server/functions/notifications';
import { getLastSeenCriticalMs } from '@/lib/notification-state';

/**
 * Hook to determine if we should show a notification dot for critical events.
 * Returns true if there are critical events in the last 7 days that haven't been seen.
 */
export function useNotificationDot() {
  const { data: hasCriticalDot, isLoading } = useQuery({
    queryKey: ['notification-critical-dot'],
    queryFn: async () => {
      if (typeof window === 'undefined') return false;

      // Server filters by priority=critical, so all returned events are critical
      const events = await listEvents({
        data: { limit: 50, priority: 'critical' },
      });
      // 7 days cutoff - critical events shouldn't vanish too quickly
      const cutoff = Date.now() - 7 * 24 * 60 * 60 * 1000;
      const lastSeen = getLastSeenCriticalMs();

      return events.some((e) => {
        const ts = e.created_at;
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
