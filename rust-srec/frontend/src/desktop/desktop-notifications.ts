import { z } from 'zod';

import {
  PRIORITY_LOW,
  PRIORITY_NORMAL,
  PRIORITY_HIGH,
  PRIORITY_CRITICAL,
} from '@/lib/priority';
import { isTauriRuntime } from '@/utils/tauri';

// Accepts integer (new) or legacy string ("low"/"normal"/"high"/"critical").
export const DesktopNotificationMinPrioritySchema = z
  .union([z.number().int().min(0).max(10), z.string()])
  .transform((v) => {
    if (typeof v === 'number') return v;
    switch (v.trim().toLowerCase()) {
      case 'low':
        return PRIORITY_LOW;
      case 'normal':
        return PRIORITY_NORMAL;
      case 'high':
        return PRIORITY_HIGH;
      case 'critical':
        return PRIORITY_CRITICAL;
      default:
        return PRIORITY_NORMAL;
    }
  });

export type DesktopNotificationMinPriority = z.infer<
  typeof DesktopNotificationMinPrioritySchema
>;

export const DesktopNotificationConfigSchema = z.object({
  enabled: z.boolean(),
  minPriority: DesktopNotificationMinPrioritySchema,
  eventTypes: z.array(z.string()),
});

export type DesktopNotificationConfig = z.infer<
  typeof DesktopNotificationConfigSchema
>;

declare global {
  interface Window {
    __RUST_SREC_DESKTOP_NOTIFICATIONS__?: unknown;
  }
}

export function readInitialDesktopNotificationsConfig(): DesktopNotificationConfig | null {
  if (typeof window === 'undefined') return null;
  const raw = window.__RUST_SREC_DESKTOP_NOTIFICATIONS__;
  const parsed = DesktopNotificationConfigSchema.safeParse(raw);
  if (!parsed.success) return null;
  return parsed.data;
}

export async function initDesktopNotificationsBridge(
  onUpdate: (config: DesktopNotificationConfig) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => {};

  const initial = readInitialDesktopNotificationsConfig();
  if (initial) onUpdate(initial);

  const { listen } = await import('@tauri-apps/api/event');
  const unlisten = await listen<DesktopNotificationConfig>(
    'rust-srec://desktop-notifications-updated',
    (event) => {
      const parsed = DesktopNotificationConfigSchema.safeParse(event.payload);
      if (parsed.success) onUpdate(parsed.data);
    },
  );

  return unlisten;
}

export async function setDesktopNotificationsConfig(
  config: DesktopNotificationConfig,
): Promise<void> {
  if (!isTauriRuntime()) return;
  const { emit } = await import('@tauri-apps/api/event');
  await emit('rust-srec://desktop-notifications-set', config);
}

export async function testDesktopNotifications(): Promise<void> {
  if (!isTauriRuntime()) return;
  const { emit } = await import('@tauri-apps/api/event');
  await emit('rust-srec://desktop-notifications-test');
}
