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

function getTauriBackendOrigin(): string | null {
  if (typeof globalThis === 'undefined') {
    return null;
  }
  const backendUrl = (
    globalThis as unknown as { __RUST_SREC_BACKEND_URL__?: unknown }
  ).__RUST_SREC_BACKEND_URL__;
  if (typeof backendUrl !== 'string' || backendUrl.trim().length === 0) {
    return null;
  }
  const normalized = backendUrl.replace(/\/$/, '');
  return normalized.startsWith('http://') || normalized.startsWith('https://')
    ? normalized
    : null;
}

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
  // or tauri:// in prod). Always prefer the runtime-injected backend origin.
  if (isTauriRuntime()) {
    const backendOrigin = getTauriBackendOrigin();
    if (backendOrigin) {
      return new URL(fullUrl, backendOrigin).toString();
    }
  }

  // Web/SSR: if API base is absolute, target backend origin directly instead of
  // resolving relative media paths against the frontend origin.
  const apiBaseUrl = getBaseUrl();
  if (apiBaseUrl.startsWith('http://') || apiBaseUrl.startsWith('https://')) {
    return new URL(fullUrl, apiBaseUrl).toString();
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
    const pathPrefix = apiBaseUrl.startsWith('/')
      ? apiBaseUrl
      : `/${apiBaseUrl}`;
    wsUrl = `${protocol}//${window.location.host}${pathPrefix}`;
  } else {
    // Fallback for SSR if no full URL is provided
    const pathPrefix = apiBaseUrl.startsWith('/')
      ? apiBaseUrl
      : `/${apiBaseUrl}`;
    wsUrl = `ws://localhost:12555${pathPrefix}`;
  }

  const basePath = wsUrl.replace(/\/$/, '');
  const path = endpoint.startsWith('/') ? endpoint : `/${endpoint}`;
  return `${basePath}${path}?token=${accessToken}`;
}

/**
 * Narrow a post-login redirect target to a path on this application.
 *
 * The value reaches us through the `redirect` search param, so it is attacker
 * controllable: anything that could resolve to another origin has to be
 * rejected. A target must therefore start with a single `/` — that also rules
 * out a scheme, which can only appear before the first `/` — and must not open
 * with `//` or `/\`, both of which browsers read as protocol-relative. Tabs,
 * newlines and carriage returns are removed from a URL before it is parsed, so
 * a value containing one is rejected rather than sanitized — a tab in
 * `/<tab>/example.com` would otherwise leave `//example.com`.
 *
 * @returns the path to navigate to, or `null` when it cannot be trusted.
 */
export function safeRedirectPath(value: unknown): string | null {
  if (typeof value !== 'string' || value.length === 0) return null;
  if (!value.startsWith('/')) return null;
  if (value.startsWith('//') || value.startsWith('/\\')) return null;
  if (value.includes('\t') || value.includes('\n') || value.includes('\r')) {
    return null;
  }
  return value;
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
