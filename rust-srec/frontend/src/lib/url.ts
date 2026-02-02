/**
 * Constructs a full URL for a media resource, handling base URLs and authentication tokens.
 * Uses relative paths to avoid SSR/client hydration mismatches.
 *
 * @param path The relative path or full URL of the media resource.
 * @param token The authentication token to append as a query parameter.
 * @returns The fully constructed URL, or null if the path is invalid.
 */
import { getBaseUrl } from '@/utils/env';
import { isTauriRuntime } from '@/utils/tauri';

export function getMediaUrl(
  path: string | null | undefined,
  token?: string,
): string | null {
  if (!path) {
    return null;
  }

  // If it's already a full URL, use it as is
  if (path.startsWith('http')) {
    return path;
  }

  // Use relative URL by default to avoid SSR/client mismatch.
  // Path from backend typically starts with /api/...
  let fullUrl = path.startsWith('/') ? path : `/${path}`;

  if (token) {
    const separator = fullUrl.includes('?') ? '&' : '?';
    fullUrl += `${separator}token=${token}`;
  }

  // Desktop/Tauri: avoid hitting the Vite dev server origin (127.0.0.1:15275 in dev
  // or tauri:// in prod). Use the runtime-injected backend URL when available.
  if (isTauriRuntime()) {
    const baseUrl = getBaseUrl();
    if (baseUrl.startsWith('http://') || baseUrl.startsWith('https://')) {
      return new URL(fullUrl, baseUrl).toString();
    }
  }

  return fullUrl;
}

/**
 * Build the WebSocket URL with JWT token as query parameter.
 * @param accessToken - JWT access token
 * @param endpoint - WebSocket endpoint path (default: /downloads/ws)
 */
export function buildWebSocketUrl(
  accessToken: string,
  endpoint: string = '/downloads/ws',
): string {
  const apiBaseUrl = getBaseUrl();

  let wsUrl: string;

  if (apiBaseUrl.startsWith('http://') || apiBaseUrl.startsWith('https://')) {
    const url = new URL(apiBaseUrl);
    const wsProtocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    wsUrl = `${wsProtocol}//${url.host}${url.pathname}`;
  } else if (typeof window !== 'undefined') {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    wsUrl = `${protocol}//${window.location.host}${apiBaseUrl}`;
  } else {
    // Fallback for SSR if no full URL is provided
    wsUrl = `ws://localhost:12555${apiBaseUrl}`;
  }

  const basePath = wsUrl.replace(/\/$/, '');
  const path = endpoint.startsWith('/') ? endpoint : `/${endpoint}`;
  return `${basePath}${path}?token=${accessToken}`;
}

/**
 * Best-effort host extraction for display purposes.
 *
 * Used to show a CDN host without exposing URL paths or query params.
 */
export function getUrlHost(url: string | null | undefined): string | null {
  if (!url) return null;
  // Fast-path: extract host without allocating URL objects.
  // We intentionally only support absolute http(s) URLs.
  const match = /^https?:\/\/([^/?#]+)/i.exec(url);
  if (!match) return null;

  // Guard against accidental userinfo leaks (e.g., http://user:pass@host).
  const hostPort = match[1];
  const at = hostPort.lastIndexOf('@');
  return at >= 0 ? hostPort.slice(at + 1) : hostPort;
}
