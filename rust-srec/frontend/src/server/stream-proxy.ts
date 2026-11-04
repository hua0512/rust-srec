import { BASE_URL } from '@/utils/env';
import { ensureValidToken } from './tokenRefresh';

const OPAQUE = 'application/octet-stream';

/**
 * The relay answers on the app origin, so an upstream page served as HTML,
 * SVG or any XML would render with the user's session. Only media, image and
 * plain data types are kept, as the bare parsed type: forwarding the raw
 * header would let `video/mp4;,text/html` through, since browsers use the last
 * type in a comma list. Everything else, including a missing type, becomes an
 * opaque download.
 */
function safeContentType(value: string | null): string {
  if (!value || value.includes(',')) return OPAQUE;
  const type = value.split(';')[0]!.trim().toLowerCase();
  if (type.endsWith('+xml')) return OPAQUE;
  if (
    /^(video|audio|image)\/[a-z0-9.+-]+$/.test(type) ||
    [
      'application/vnd.apple.mpegurl',
      'application/x-mpegurl',
      'application/json',
      'text/plain',
    ].includes(type)
  ) {
    return type;
  }
  return OPAQUE;
}

/**
 * Browser-facing relay. The backend owns source configuration, target/redirect
 * validation, upstream transport and HLS rewriting for both web and desktop.
 */
export async function handleStreamProxyRequest(
  request: Request,
): Promise<Response> {
  // Players and <img> tags load the relay from the app itself (`same-origin`)
  // and a typed URL sends `none`; any other site, sibling subdomains included,
  // would be borrowing the viewer's session. Plain-HTTP origins send no header.
  const fetchSite = request.headers.get('Sec-Fetch-Site');
  if (
    fetchSite !== null &&
    fetchSite !== 'same-origin' &&
    fetchSite !== 'none'
  ) {
    return new Response('Forbidden', { status: 403 });
  }
  const user = await ensureValidToken();
  if (!user) return new Response('Unauthorized', { status: 401 });

  const incoming = new URL(request.url);
  const target = incoming.searchParams.get('url');
  const handle = incoming.searchParams.get('playback_handle');
  const stream = incoming.searchParams.get('stream');
  const query = new URLSearchParams({ web: 'true' });
  if (handle !== null || stream !== null) {
    // The backend attaches the handle's account headers; client-supplied
    // headers or cookies never accompany a managed request.
    if (
      !handle ||
      !stream ||
      !target ||
      ['headers', 'source_url', 'cookies', 'credential_id'].some((name) =>
        incoming.searchParams.has(name),
      )
    )
      return new Response('Invalid managed playback request', { status: 400 });
    query.set('url', target);
    query.set('playback_handle', handle);
    query.set('stream', stream);
  } else {
    if (!target) return new Response('Missing url parameter', { status: 400 });
    query.set('url', target);
    for (const name of ['headers', 'source_url']) {
      const value = incoming.searchParams.get(name);
      if (value != null) query.set(name, value);
    }
  }
  const headers = new Headers({
    Authorization: `Bearer ${user.token.access_token}`,
  });
  const range = request.headers.get('Range');
  if (range) headers.set('Range', range);

  try {
    // The backend emits /stream-proxy links without a bearer token when web=true.
    // Never follow a backend redirect carrying the session credential.
    const response = await fetch(
      `${BASE_URL.replace(/\/$/, '')}/stream-proxy?${query}`,
      {
        headers,
        redirect: 'manual',
        signal: request.signal,
      },
    );
    if (response.status >= 300 && response.status < 400) {
      await response.body?.cancel();
      return new Response('Unexpected backend redirect', { status: 502 });
    }
    const outputHeaders = new Headers({
      'Cache-Control': 'private, no-store',
      'Referrer-Policy': 'no-referrer',
      'X-Content-Type-Options': 'nosniff',
      // Scripts off and an opaque origin, while a video opened directly in a
      // tab can still load itself.
      'Content-Security-Policy': 'sandbox',
      'Content-Type': safeContentType(response.headers.get('Content-Type')),
    });
    for (const name of ['Content-Length', 'Content-Range', 'Accept-Ranges']) {
      const value = response.headers.get(name);
      if (value) outputHeaders.set(name, value);
    }
    return new Response(response.body, {
      status: response.status,
      headers: outputHeaders,
    });
  } catch (error) {
    if (
      request.signal.aborted ||
      (error instanceof Error && error.name === 'AbortError')
    ) {
      return new Response(null, { status: 499 });
    }
    // Fetch errors can contain URLs and credentials; expose only a stable message.
    return new Response('Stream proxy backend is unavailable', { status: 502 });
  }
}
