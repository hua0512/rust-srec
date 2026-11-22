import { z } from 'zod';

import { isDesktopBuild } from '@/utils/desktop';
import type { DesktopBackendMode } from '@/utils/session-storage';
import { isTauriRuntime } from '@/utils/tauri';

const DesktopBackendModeSchema = z.enum([
  'local',
  'remote',
]) satisfies z.ZodType<DesktopBackendMode>;

export const DesktopSettingsSchema = z.object({
  startMinimized: z.boolean(),
  closeToTray: z.boolean(),
  minimizeToTray: z.boolean(),
  preventSleepWhileRecording: z.boolean(),
  confirmExitWhileRecording: z.boolean(),
  requireLogin: z.boolean(),
  checkUpdatesOnStartup: z.boolean(),
  skippedUpdateVersion: z.string().nullable(),
  backendMode: DesktopBackendModeSchema,
  remoteUrl: z.string().nullable(),
});

export type DesktopSettings = z.infer<typeof DesktopSettingsSchema>;

const DesktopSettingsSnapshotSchema = DesktopSettingsSchema.extend({
  autostartEnabled: z.boolean(),
  dataDir: z.string(),
  logDir: z.string(),
  outputDir: z.string().nullable(),
  activeBackendMode: DesktopBackendModeSchema,
  appVersion: z.string(),
  /** The local account still has its initial password. */
  passwordSetupRequired: z.boolean(),
  /** The account the app signs in as automatically; null in remote mode. */
  localUsername: z.string().nullable(),
});

export type DesktopSettingsSnapshot = z.infer<
  typeof DesktopSettingsSnapshotSchema
>;

const DesktopToolStatusSchema = z.object({
  id: z.string(),
  name: z.string(),
  configuredPath: z.string(),
  status: z.enum(['available', 'outdated', 'missing', 'error']),
  version: z.string().nullable(),
  detail: z.string().nullable(),
});

export type DesktopToolStatus = z.infer<typeof DesktopToolStatusSchema>;

const DesktopUpdateInfoSchema = z.object({
  currentVersion: z.string(),
  latestVersion: z.string(),
  updateAvailable: z.boolean(),
  releaseUrl: z.string(),
  publishedAt: z.string().nullable(),
  notesExcerpt: z.string().nullable(),
});

export type DesktopUpdateInfo = z.infer<typeof DesktopUpdateInfoSchema>;

const RemoteConnectionCheckSchema = z.object({
  normalizedUrl: z.string(),
  healthy: z.boolean(),
  statusCode: z.number(),
  detail: z.string().nullable(),
});

export type RemoteConnectionCheck = z.infer<typeof RemoteConnectionCheckSchema>;

type DesktopSystemFolderLocation = 'data' | 'logs';

const DESKTOP_SETTINGS_UPDATED_EVENT = 'rust-srec://desktop-settings-updated';
const DESKTOP_UPDATE_AVAILABLE_EVENT = 'rust-srec://desktop-update-available';

function assertTauriRuntime(): void {
  if (!isDesktopBuild() || !isTauriRuntime()) {
    throw new Error('Desktop settings are only available in the desktop app.');
  }
}

/**
 * Invokes a desktop shell command and validates its reply. The Tauri API is
 * imported lazily, after the desktop-build check.
 */
async function invokeDesktop<T>(
  command: string,
  args: Record<string, unknown> | undefined,
  schema: z.ZodType<T>,
): Promise<T> {
  assertTauriRuntime();
  const { invoke } = await import('@tauri-apps/api/core');
  return schema.parse(await invoke<unknown>(command, args));
}

const NoReplySchema = z.unknown().transform((): void => undefined);

export function getDesktopSettings(): Promise<DesktopSettingsSnapshot> {
  return invokeDesktop(
    'get_desktop_settings',
    undefined,
    DesktopSettingsSnapshotSchema,
  );
}

export async function updateDesktopSettings(
  settings: DesktopSettings,
): Promise<DesktopSettings> {
  return invokeDesktop(
    'update_desktop_settings',
    { settings: DesktopSettingsSchema.parse(settings) },
    DesktopSettingsSchema,
  );
}

/**
 * Replaces the local account's initial password and resolves to its username.
 * The shell refuses once the account has a password of its own. Setting it
 * signs out every session of that account, this one included.
 */
export function setDesktopPassword(password: string): Promise<string> {
  return invokeDesktop('set_desktop_password', { password }, z.string());
}

export function setDesktopAutostartEnabled(enabled: boolean): Promise<boolean> {
  return invokeDesktop('set_autostart_enabled', { enabled }, z.boolean());
}

export function openDesktopRecordingsFolder(): Promise<void> {
  return invokeDesktop('open_recordings_folder', undefined, NoReplySchema);
}

export function openDesktopSystemFolder(
  location: DesktopSystemFolderLocation,
): Promise<void> {
  return invokeDesktop(
    'open_desktop_recovery_location',
    { location },
    NoReplySchema,
  );
}

export function checkDesktopTools(): Promise<DesktopToolStatus[]> {
  return invokeDesktop(
    'check_desktop_tools',
    undefined,
    z.array(DesktopToolStatusSchema),
  );
}

export function checkForDesktopUpdates(): Promise<DesktopUpdateInfo> {
  return invokeDesktop('check_for_updates', undefined, DesktopUpdateInfoSchema);
}

export function skipDesktopUpdateVersion(
  version: string | null,
): Promise<DesktopSettings> {
  return invokeDesktop(
    'skip_update_version',
    { version },
    DesktopSettingsSchema,
  );
}

export function testRemoteBackendConnection(
  url: string,
): Promise<RemoteConnectionCheck> {
  return invokeDesktop(
    'test_remote_backend_connection',
    { url },
    RemoteConnectionCheckSchema,
  );
}

export function restartDesktopApp(): Promise<void> {
  return invokeDesktop('restart_desktop', undefined, NoReplySchema);
}

export async function openDesktopExternalUrl(url: string): Promise<void> {
  assertTauriRuntime();
  const { openUrl } = await import('@tauri-apps/plugin-opener');
  await openUrl(url);
}

export async function listenDesktopSettingsUpdated(
  onUpdate: (settings: DesktopSettings) => void,
): Promise<() => void> {
  if (!isDesktopBuild() || !isTauriRuntime()) return () => {};

  const { listen } = await import('@tauri-apps/api/event');
  return await listen<unknown>(DESKTOP_SETTINGS_UPDATED_EVENT, (event) => {
    const parsed = DesktopSettingsSchema.safeParse(event.payload);
    if (parsed.success) {
      onUpdate(parsed.data);
    }
  });
}

export async function listenDesktopUpdateAvailable(
  onUpdate: (info: DesktopUpdateInfo) => void,
): Promise<() => void> {
  if (!isDesktopBuild() || !isTauriRuntime()) return () => {};

  const { listen } = await import('@tauri-apps/api/event');
  return await listen<unknown>(DESKTOP_UPDATE_AVAILABLE_EVENT, (event) => {
    const parsed = DesktopUpdateInfoSchema.safeParse(event.payload);
    if (parsed.success) {
      onUpdate(parsed.data);
    }
  });
}
