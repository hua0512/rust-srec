export const IS_DESKTOP_BUILD = import.meta.env.VITE_DESKTOP === '1';

export function isDesktopBuild(): boolean {
  return IS_DESKTOP_BUILD;
}
