import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { listEvents } from '@/server/functions/notifications';
import type { NotificationEventLog } from '@/api/schemas/notifications';
import {
  getBrowserNotificationsEnabled,
  getLastNotifiedCriticalMs,
  getLastSeenCriticalMs,
  onNotificationStateChanged,
  setLastNotifiedCriticalMs,
} from '@/lib/notification-state';

function safeParseJson(input: string): any | null {
  try {
    return JSON.parse(input);
  } catch {
    return null;
  }
}

function formatBrowserNotification(log: NotificationEventLog): {
  title: string;
  body?: string;
} {
  const baseTitle = `rust-srec: ${log.event_type}`;
  const parsed = safeParseJson(log.payload);
  if (!parsed || typeof parsed !== 'object') {
    return { title: baseTitle, body: log.payload?.slice(0, 200) };
  }

  const variant = Object.keys(parsed)[0];
  const inner = variant ? (parsed as any)[variant] : null;
  const streamer = inner?.streamer_name ?? inner?.streamer ?? inner?.streamerId;
  const error =
    inner?.error_message ??
    inner?.error ??
    inner?.message ??
    inner?.reason ??
    inner?.errorMessage;

  const bodyParts: string[] = [];
  if (streamer) bodyParts.push(String(streamer));
  if (error) bodyParts.push(String(error));
  if (bodyParts.length === 0) bodyParts.push(log.priority);

  return { title: baseTitle, body: bodyParts.join(' — ').slice(0, 200) };
}

export function BrowserNotificationListener() {
  const isSupported = typeof window !== 'undefined' && 'Notification' in window;

  const [enabled, setEnabled] = useState(false);
  const [permission, setPermission] =
    useState<NotificationPermission>('default');

  useEffect(() => {
    if (!isSupported) return;
    const sync = () => {
      setEnabled(getBrowserNotificationsEnabled());
      setPermission(Notification.permission);
    };
    sync();
    return onNotificationStateChanged(sync);
  }, [isSupported]);

  const shouldPoll = useMemo(() => {
    return isSupported && enabled && permission === 'granted';
  }, [enabled, isSupported, permission]);

  const { data: events } = useQuery({
    queryKey: ['notification-events', 'browser', 'critical'],
    queryFn: () =>
      listEvents({ data: { limit: 50, offset: 0, priority: 'critical' } }),
    enabled: shouldPoll,
    refetchInterval: 60_000,
    refetchOnWindowFocus: true,
  });

  useEffect(() => {
    if (!shouldPoll) return;
    if (!events || events.length === 0) return;

    const lastSeen = getLastSeenCriticalMs();
    const lastNotifiedRaw = getLastNotifiedCriticalMs();
    const baseline = Math.max(lastSeen, lastNotifiedRaw || 0);

    // Avoid spamming old events when enabling for the first time.
    if (!lastNotifiedRaw) {
      setLastNotifiedCriticalMs(Date.now());
      return;
    }

    // Server already filters by priority=critical, so all events are critical
    const candidates = events
      .map((e) => ({ e, t: e.created_at }))
      .filter((x) => Number.isFinite(x.t) && x.t > baseline)
      .sort((a, b) => a.t - b.t);

    if (candidates.length === 0) return;

    let maxNotified = baseline;
    for (const { e, t } of candidates) {
      const { title, body } = formatBrowserNotification(e);
      try {
        const n = new Notification(title, {
          body,
          tag: `rust-srec:${e.id}`,
          requireInteraction: true,
        });
        n.onclick = () => {
          try {
            window.focus();
            window.location.assign('/notifications/events');
          } catch {
            // ignore
          }
        };
      } catch {
        // ignore
      }
      if (t > maxNotified) maxNotified = t;
    }

    setLastNotifiedCriticalMs(maxNotified);
  }, [events, shouldPoll]);

  return null;
}
