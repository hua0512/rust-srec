import { isDesktopBuild } from '@/utils/desktop';

const WINDOWS_DRIVE_PREFIX = /^[a-zA-Z]:[/\\]/;

export interface PickDesktopPathOptions {
  mode: 'directory' | 'file';
  defaultPath?: string;
}

/**
 * Returns a trimmed path only when `value` looks like an actual filesystem path,
 * filtering out empty strings, unexpanded `~` shortcuts, and bare binary names
 * such as `'ffmpeg'` or `'streamlink'`.
 */
function resolveDialogDefaultPath(
  value: string | undefined,
): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed || trimmed.startsWith('~')) return undefined;
  if (
    trimmed.includes('/') ||
    trimmed.includes('\\') ||
    WINDOWS_DRIVE_PREFIX.test(trimmed)
  ) {
    return trimmed;
  }
  return undefined;
}

/**
 * Opens a native Tauri directory or file picker dialog.
 * Guarded by `isDesktopBuild()` so web builds tree-shake `@tauri-apps/*`.
 */
export async function pickDesktopPath(
  options: PickDesktopPathOptions,
): Promise<string | null> {
  if (!isDesktopBuild()) return null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected: string | string[] | null = await open({
    directory: options.mode === 'directory',
    multiple: false,
    defaultPath: resolveDialogDefaultPath(options.defaultPath),
  });
  if (Array.isArray(selected)) {
    return selected[0] ?? null;
  }
  return selected;
}

/**
 * Opens an existing directory in the OS file manager via the Tauri backend command.
 * Guarded by `isDesktopBuild()` so web builds tree-shake `@tauri-apps/*`.
 */
export async function openDesktopDirectory(path: string): Promise<void> {
  if (!isDesktopBuild()) return;
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('open_directory_in_explorer', { path });
}
