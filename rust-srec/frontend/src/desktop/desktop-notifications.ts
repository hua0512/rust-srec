import { z } from 'zod';

import { isTauriRuntime } from '@/utils/tauri';

export const DesktopNotificationMinPrioritySchema = z.enum([
  'low',
  'normal',
  'high',
  'critical',
]);

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
