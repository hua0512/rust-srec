import { isDesktopBuild } from '@/utils/desktop';

export function isTauriRuntime(): boolean {
  if (typeof window === 'undefined') return false;
  const w = window as unknown as {
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: unknown;
  };
  return (
    typeof w.__TAURI__ !== 'undefined' ||
    typeof w.__TAURI_INTERNALS__ !== 'undefined'
  );
}

/**
 * Select `path` in the system file manager.
 *
 * Only meaningful in the desktop build, where the recording already sits on this
 * machine. Guarded by `isDesktopBuild()` so Rollup strips `@tauri-apps/*` from
 * the web bundle, and the promise rejects when the item is gone.
 */
export async function revealItemInDir(path: string): Promise<void> {
  if (!isDesktopBuild()) {
    throw new Error('Only available in the desktop app.');
  }
  const { revealItemInDir: reveal } = await import('@tauri-apps/plugin-opener');
  await reveal(path);
}

/**
 * Open a recorded media output by ID in the system default media player.
 *
 * The backend looks up the output ID in SQLite and verifies the file is a
 * playable media output on disk before handing it to the OS opener.
 */
export async function openRecordingInPlayer(outputId: string): Promise<void> {
  if (!isDesktopBuild()) {
    throw new Error('Only available in the desktop app.');
  }
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('open_recording_in_player', { outputId });
}
