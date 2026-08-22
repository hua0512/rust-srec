interface ImportMetaEnv {
  readonly VITE_UI_BUILD?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

// Injected via `define` in vite.config.ts / vite.desktop.config.ts (content
// hash of the theming sources — see theme-cache-id.ts). Undefined under
// vitest and when lib/theme-script.ts is imported by the desktop Vite config
// itself (getThemeCacheId falls back to 'dev').
declare const __THEME_CACHE_ID__: string | undefined;
