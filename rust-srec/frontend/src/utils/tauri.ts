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
 * machine. The plugin is imported lazily so it stays out of the web bundle, and
 * the promise rejects when the item is gone — callers surface the message.
 */
export async function revealItemInDir(path: string): Promise<void> {
  const { revealItemInDir: reveal } = await import('@tauri-apps/plugin-opener');
  await reveal(path);
}
