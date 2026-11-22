import { z } from 'zod';

import { isDesktopBuild } from '@/utils/desktop';
import { getDesktopBackendMode } from '@/utils/session-storage';
import { isTauriRuntime } from '@/utils/tauri';

export const DesktopAutoLoginPayloadSchema = z.object({
  username: z.string().min(1),
  accessToken: z.string().min(1),
  refreshToken: z.string().min(1),
  expiresIn: z.number(),
  refreshExpiresIn: z.number(),
  roles: z.array(z.string()),
  mustChangePassword: z.boolean(),
});

export type DesktopAutoLoginPayload = z.infer<
  typeof DesktopAutoLoginPayloadSchema
>;

// In-memory flag so an explicit Logout in desktop local mode stays on /login
// instead of immediately auto-logging back in until the user signs in again
// or restarts the app.
let explicitDesktopLogout = false;

export function hasExplicitDesktopLogout(): boolean {
  return explicitDesktopLogout;
}

export function markExplicitDesktopLogout(): void {
  explicitDesktopLogout = true;
}

export function clearExplicitDesktopLogout(): void {
  explicitDesktopLogout = false;
}

export function isLocalDesktopBackend(): boolean {
  return isTauriRuntime() && getDesktopBackendMode() === 'local';
}

export async function requestDesktopAutoLogin(): Promise<DesktopAutoLoginPayload | null> {
  if (
    !isDesktopBuild() ||
    !isTauriRuntime() ||
    getDesktopBackendMode() !== 'local'
  ) {
    return null;
  }

  const { invoke } = await import('@tauri-apps/api/core');
  const raw = await invoke<unknown>('desktop_auto_login');
  if (raw === null || raw === undefined) {
    return null;
  }

  const parsed = DesktopAutoLoginPayloadSchema.safeParse(raw);
  if (!parsed.success) {
    console.warn(
      '[desktop] Invalid desktop_auto_login payload:',
      parsed.error.message,
    );
    return null;
  }

  return parsed.data;
}
