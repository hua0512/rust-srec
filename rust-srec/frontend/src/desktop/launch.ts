export type DesktopLaunchPayload = {
  args: string[];
  cwd: string;
};

declare global {
  interface Window {
    __RUST_SREC_LAUNCH_ARGS__?: unknown;
    __RUST_SREC_LAUNCH_CWD__?: unknown;
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: unknown;
  }
}

function isTauriRuntime(): boolean {
  if (typeof window === 'undefined') return false;
  return typeof window.__TAURI__ !== 'undefined' || typeof window.__TAURI_INTERNALS__ !== 'undefined';
}

function readInitialLaunchPayload(): DesktopLaunchPayload | null {
  if (typeof window === 'undefined') return null;

  const rawArgs = window.__RUST_SREC_LAUNCH_ARGS__;
  const rawCwd = window.__RUST_SREC_LAUNCH_CWD__;

  const args = Array.isArray(rawArgs) && rawArgs.every((v) => typeof v === 'string') ? rawArgs : null;
  const cwd = typeof rawCwd === 'string' ? rawCwd : '';
  if (!args) return null;

  return { args, cwd };
}

export async function initDesktopLaunchListener(
  onLaunch: (payload: DesktopLaunchPayload) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => {};

  const initial = readInitialLaunchPayload();
  if (initial) {
    onLaunch(initial);
  }

  const { listen } = await import('@tauri-apps/api/event');
  const unlisten = await listen<DesktopLaunchPayload>('rust-srec://single-instance', (event) => {
    onLaunch(event.payload);
  });

  return unlisten;
}
