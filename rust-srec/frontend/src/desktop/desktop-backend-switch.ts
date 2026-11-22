import { clearDesktopClientSessionFn, logoutFn } from '@/server/functions/auth';
import { isDesktopBuild } from '@/utils/desktop';
import { getDesktopBackendMode } from '@/utils/session-storage';
import { isTauriRuntime } from '@/utils/tauri';

// An unreachable server must not hold up the switch, so the sign-out request
// gets this long before the switch goes ahead without it.
const REMOTE_LOGOUT_TIMEOUT_MS = 3_000;

/**
 * Drops the stored session before the desktop app changes backend. When the
 * active backend is a remote server, its refresh token is revoked there first
 * on a best-effort basis; otherwise it would stay valid on that server long
 * after this app forgot it.
 */
export async function endDesktopBackendSession(): Promise<void> {
  if (getDesktopBackendMode() === 'remote') {
    await Promise.race([
      logoutFn().catch((error: unknown) => {
        console.warn('[desktop] Remote sign-out failed (ignoring):', error);
      }),
      new Promise<void>((resolve) =>
        setTimeout(resolve, REMOTE_LOGOUT_TIMEOUT_MS),
      ),
    ]);
  }
  await clearDesktopClientSessionFn();
}

export async function switchDesktopToLocalBackend(): Promise<void> {
  if (!isDesktopBuild() || !isTauriRuntime()) {
    throw new Error(
      'Switching to the local backend is only available in the desktop app.',
    );
  }

  await endDesktopBackendSession();
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('switch_to_local_backend');
}
