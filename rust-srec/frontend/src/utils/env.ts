export const getBaseUrl = () => {
  // Tauri desktop: runtime-injected backend URL (set by the native host).
  // Keep this synchronous because it is used at import time by existing modules.
  if (typeof globalThis !== 'undefined') {
    const backendUrl = (globalThis as unknown as { __RUST_SREC_BACKEND_URL__?: unknown })
      .__RUST_SREC_BACKEND_URL__;
    if (typeof backendUrl === 'string' && backendUrl.trim().length > 0) {
      return `${backendUrl.replace(/\/$/, '')}/api`;
    }

    // Dev-only hint: without injection, desktop ends up using `window.location` (Vite dev server
    // in dev, `tauri://` in prod), which breaks WebSocket endpoints.
    if (
      import.meta.env.DEV &&
      typeof window !== 'undefined' &&
      typeof (window as any).__TAURI__ !== 'undefined'
    ) {
      console.warn(
        '[desktop] __RUST_SREC_BACKEND_URL__ missing; falling back to VITE_API_BASE_URL/window.location. WebSockets will likely fail.',
      );
    }
  }

  // Check for server-side environment variables first (for SSR)
  if (typeof process !== 'undefined' && process.env) {
    if (process.env.API_BASE_URL) return process.env.API_BASE_URL;
    if (process.env.BACKEND_URL) return `${process.env.BACKEND_URL}/api`;
  }
  // Fallback to relative path for client-side or if no env var is set
  return import.meta.env.VITE_API_BASE_URL || '/api';
};

export const BASE_URL = getBaseUrl();
