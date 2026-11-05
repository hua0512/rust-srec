import { useEffect, useState } from 'react';
import { useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import { listEvents } from '@/server/functions/notifications';
import type { NotificationEventLog } from '@/api/schemas/notifications';
import { useLingui } from '@lingui/react';
import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { priorityLabel } from '@/lib/priority';
import {
  getBrowserNotificationsEnabled,
  getLastNotifiedCriticalMs,
  getLastSeenCriticalMs,
  onNotificationStateChanged,
  setLastNotifiedCriticalMs,
} from '@/lib/notification-state';

type PayloadFields = Record<string, unknown>;

function isRecord(value: unknown): value is PayloadFields {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

/**
 * The fields of a stored event, or `null` when the payload is not a JSON object.
 * The backend stores its externally tagged event, `{ Variant: { ...fields } }`;
 * credential events nest their fields one level deeper, under `event`.
 */
function payloadFields(payload: string): PayloadFields | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(payload);
  } catch {
    return null;
  }
  if (!isRecord(parsed)) return null;
  const [variant] = Object.keys(parsed);
  const body = variant ? parsed[variant] : undefined;
  if (!isRecord(body)) return {};
  return variant === 'Credential' && isRecord(body.event) ? body.event : body;
}

/**
 * Takes `i18n` rather than reading it from context: this runs from the polling effect below,
 * outside any component render, so `useLingui` is not available at the call site.
 */
export function formatBrowserNotification(
  log: NotificationEventLog,
  i18n: I18n,
): {
  title: string;
  body?: string;
} {
  const baseTitle = `rust-srec: ${log.event_type}`;
  const fields = payloadFields(log.payload);
  if (!fields) {
    return { title: baseTitle, body: log.payload?.slice(0, 200) };
  }

  const streamer = text(fields.streamer_name);
  const error =
    text(fields.error_message) ??
    text(fields.error) ??
    text(fields.message) ??
    text(fields.reason);
  const platform = text(fields.platform);
  const path = text(fields.path);

  const bodyParts: string[] = [];
  if (streamer) bodyParts.push(streamer);
  // Names the platform of a failed account sign-in.
  if (platform) bodyParts.push(platform);
  // Names the output folder that cannot be written.
  if (path) bodyParts.push(path);
  // The queue event's `threshold` is not the configured alert level, so only
  // the queue length is shown.
  if (typeof fields.queue_depth === 'number') {
    const queued = fields.queue_depth;
    bodyParts.push(i18n._(msg`${queued} jobs queued`));
  }
  if (error) bodyParts.push(error);
  if (bodyParts.length === 0)
    bodyParts.push(i18n._(priorityLabel(log.priority)));

  return { title: baseTitle, body: bodyParts.join(' — ').slice(0, 200) };
}

export function BrowserNotificationListener() {
  const { i18n } = useLingui();
  const navigate = useNavigate();
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

  const shouldPoll = isSupported && enabled && permission === 'granted';

  const { data: events } = useQuery({
    queryKey: queryKeys.notifications.browserCriticalEvents(),
    queryFn: () =>
      listEvents({ data: { limit: 50, offset: 0, priority: '10' } }),
    enabled: shouldPoll,
    refetchInterval: 60_000,
    // Browser notifications matter most while the tab is in the background,
    // so this poll is exempt from the app-wide pause for hidden tabs.
    refetchIntervalInBackground: true,
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
      const { title, body } = formatBrowserNotification(e, i18n);
      try {
        const n = new Notification(title, {
          body,
          tag: `rust-srec:${e.id}`,
          requireInteraction: true,
        });
        n.onclick = () => {
          try {
            window.focus();
            // Routing through the router rather than the address bar: the
            // desktop build serves the app from hash history, where a
            // pathname navigation lands on the index route instead.
            void navigate({ to: '/notifications/events' });
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
  }, [events, shouldPoll, i18n, navigate]);

  return null;
}
