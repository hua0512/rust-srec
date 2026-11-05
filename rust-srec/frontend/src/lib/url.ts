import { getPlatformFromUrl } from '@/lib/platform';
import { isDesktopBuild } from '@/utils/desktop';
import { BASE_URL, getBaseUrl } from '@/utils/env';
import { getDesktopAccessToken } from '@/utils/session';

/** `path` under `base`, whether or not `base` ends in a slash. */
export function joinUrlPath(base: string, path: string): string {
  const trimmed = base.endsWith('/') ? base.slice(0, -1) : base;
  return `${trimmed}${path.startsWith('/') ? path : `/${path}`}`;
}

export interface StreamProxyTarget {
  desktopBuild: boolean;
  desktopToken: string | null;
  baseUrl: string;
}

/**
 * The stream proxy URL that fetches what `query` describes. The web build
 * serves the proxy from its own origin. The desktop build has no frontend
 * server, so its requests go to the backend and carry the session token; it
 * gets `null` without one.
 */
export function streamProxyUrl(
  query: URLSearchParams,
  { desktopBuild, desktopToken, baseUrl }: StreamProxyTarget,
): string | null {
  if (!desktopBuild) return `/stream-proxy?${query}`;
  if (!desktopToken) return null;
  const withToken = new URLSearchParams(query);
  withToken.set('token', desktopToken);
  return `${joinUrlPath(baseUrl, '/stream-proxy')}?${withToken}`;
}

/**
 * Image hosts of these platforms refuse requests that do not name the
 * platform's site as the referer, so their images load through the proxy.
 */
const IMAGE_REFERERS: Partial<Record<string, string>> = {
  acfun: 'https://live.acfun.cn/',
  bilibili: 'https://live.bilibili.com/',
  douyin: 'https://live.douyin.com/',
  douyu: 'https://www.douyu.com/',
  huya: 'https://www.huya.com/',
};

/**
 * An image URL the browser can load: proxied with the referer its platform
 * requires, or unchanged when it needs none or a desktop build has no session
 * to proxy with.
 */
export function getProxiedUrl(url: string | null | undefined) {
  if (!url) return undefined;
  const referer = IMAGE_REFERERS[getPlatformFromUrl(url)];
  if (!referer) return url;

  const desktopBuild = isDesktopBuild();
  const query = new URLSearchParams({
    url,
    headers: JSON.stringify({ Referer: referer }),
  });
  return (
    streamProxyUrl(query, {
      desktopBuild,
      desktopToken: desktopBuild ? getDesktopAccessToken() : null,
      baseUrl: BASE_URL,
    }) ?? url
  );
}

/**
 * Constructs a full URL for a media resource, handling base URLs and authentication tokens.
 * Uses relative paths to avoid SSR/client hydration mismatches.
 *
 * @param path The relative path or full URL of the media resource.
 * @param token The authentication token to append as a query parameter.
 * @returns The fully constructed URL, or null if the path is invalid.
 */
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

  // Absolute API bases, including the runtime-injected desktop backend, resolve
  // media paths against the backend origin rather than the frontend origin.
  const apiBaseUrl = getBaseUrl();
  if (apiBaseUrl.startsWith('http://') || apiBaseUrl.startsWith('https://')) {
    return new URL(fullUrl, apiBaseUrl).toString();
  }

  return fullUrl;
}

/**
 * Build the URL that saves a media output to disk.
 *
 * Same-origin an anchor's `download` attribute is enough, so the plain media URL
 * is returned. Cross-origin the attribute is ignored, and the request instead
 * asks the media route for `Content-Disposition: attachment`: the browser then
 * streams the response to disk without navigating away and without the
 * application ever holding the file — a recording can run to several gigabytes.
 *
 * Returns `null` when no URL can be built.
 */
export function getMediaDownloadUrl(
  outputId: string,
  token?: string,
): string | null {
  const url = getMediaUrl(
    `/api/media/${encodeURIComponent(outputId)}/content`,
    token,
  );
  if (!url || isSameOriginUrl(url)) {
    return url;
  }
  const separator = url.includes('?') ? '&' : '?';
  return `${url}${separator}download=1`;
}

/**
 * Build the WebSocket URL with JWT token as query parameter.
 * @param accessToken - JWT access token
 * @param endpoint - WebSocket endpoint path, optionally with a query string
 *   (default: /downloads/ws)
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
  const separator = path.includes('?') ? '&' : '?';
  return `${basePath}${path}${separator}token=${accessToken}`;
}

/**
 * True when `url` resolves to the page's own origin.
 *
 * An anchor's `download` attribute is honoured only for a same-origin URL. A
 * cross-origin media link is otherwise an ordinary top-level navigation that
 * replaces the application with the raw file — which is what the desktop build
 * and any deployment with an absolute API base would do, because `getMediaUrl`
 * returns the backend's own origin there.
 *
 * Returns `false` off the browser and for a URL that cannot be resolved.
 */
export function isSameOriginUrl(url: string): boolean {
  if (typeof window === 'undefined') return false;
  try {
    return new URL(url, window.location.href).origin === window.location.origin;
  } catch {
    return false;
  }
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
