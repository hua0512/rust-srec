import { BASE_URL } from '@/utils/env';
import { ensureValidToken } from './tokenRefresh';

/**
 * Browser-facing relay. The backend owns source configuration, target/redirect
 * validation, upstream transport and HLS rewriting for both web and desktop.
 */
export async function handleStreamProxyRequest(
  request: Request,
): Promise<Response> {
  const user = await ensureValidToken();
  if (!user) return new Response('Unauthorized', { status: 401 });

  const incoming = new URL(request.url);
  const target = incoming.searchParams.get('url');
  if (!target) return new Response('Missing url parameter', { status: 400 });
  const query = new URLSearchParams({ url: target, web: 'true' });
  for (const name of ['headers', 'source_url']) {
    const value = incoming.searchParams.get(name);
    if (value != null) query.set(name, value);
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
    const outputHeaders = new Headers({ 'Cache-Control': 'private, no-store' });
    for (const name of [
      'Content-Type',
      'Content-Length',
      'Content-Range',
      'Accept-Ranges',
    ]) {
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
