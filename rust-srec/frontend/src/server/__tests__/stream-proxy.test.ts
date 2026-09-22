const ensureValidTokenMock = vi.hoisted(() => vi.fn());
vi.mock('../tokenRefresh', () => ({ ensureValidToken: ensureValidTokenMock }));
vi.mock('@/utils/env', () => ({ BASE_URL: 'http://backend.example/api/' }));
import { handleStreamProxyRequest } from '../stream-proxy';

const fetchMock = vi.fn<typeof fetch>();
beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockReset();
  ensureValidTokenMock.mockResolvedValue({
    token: { access_token: 'server-session' },
  });
});
afterEach(() => vi.unstubAllGlobals());

it('requires a web session before contacting the backend', async () => {
  ensureValidTokenMock.mockResolvedValue(null);
  const response = await handleStreamProxyRequest(
    new Request(
      'https://app.example/stream-proxy?url=https://cdn.example/live',
    ),
  );
  expect(response.status).toBe(401);
  expect(fetchMock).not.toHaveBeenCalled();
});

it('forwards identity, source headers and ranges only to the backend, with server-side authentication', async () => {
  fetchMock.mockResolvedValue(
    new Response('ab', {
      status: 206,
      headers: {
        'Content-Type': 'video/mp2t',
        'Content-Range': 'bytes 0-1/3',
        'Accept-Ranges': 'bytes',
        'Set-Cookie': 'untrusted=value',
      },
    }),
  );
  const query = new URLSearchParams({
    url: 'https://cdn.example/video.ts?signature=abc',
    source_url: 'https://source.example/channel',
    headers: '{"Cookie":"source-session"}',
    token: 'untrusted-token',
    web: 'false',
  });
  const request = new Request(`https://app.example/stream-proxy?${query}`, {
    headers: {
      Range: 'bytes=0-1',
      Authorization: 'untrusted',
      Cookie: 'browser-cookie',
    },
  });
  const response = await handleStreamProxyRequest(request);
  const [url, init] = fetchMock.mock.calls[0]!;
  const backend = new URL(url as string);
  expect(backend.origin).toBe('http://backend.example');
  expect(backend.pathname).toBe('/api/stream-proxy');
  expect(backend.searchParams.get('source_url')).toBe(
    'https://source.example/channel',
  );
  expect(backend.searchParams.get('url')).toBe(query.get('url'));
  expect(backend.searchParams.get('headers')).toBe(query.get('headers'));
  expect(backend.searchParams.get('web')).toBe('true');
  expect(backend.searchParams.has('token')).toBe(false);
  const headers = new Headers(init!.headers);
  expect(headers.get('Authorization')).toBe('Bearer server-session');
  expect(headers.get('Range')).toBe('bytes=0-1');
  expect(headers.has('Cookie')).toBe(false);
  expect(init!.redirect).toBe('manual');
  expect(init!.signal).toBe(request.signal);
  expect(response.status).toBe(206);
  expect(response.headers.get('Content-Range')).toBe('bytes 0-1/3');
  expect(response.headers.get('Cache-Control')).toBe('private, no-store');
  expect(response.headers.has('Set-Cookie')).toBe(false);
  expect(await response.text()).toBe('ab');
});

it('returns streaming bodies without waiting for the live response to finish', async () => {
  let streamController: ReadableStreamDefaultController<Uint8Array> | undefined;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      streamController = controller;
    },
  });
  fetchMock.mockResolvedValue(new Response(body));
  const response = await handleStreamProxyRequest(
    new Request(
      'https://app.example/stream-proxy?url=https://cdn.example/live',
    ),
  );
  const reader = response.body!.getReader();
  streamController!.enqueue(new TextEncoder().encode('live chunk'));
  expect(new TextDecoder().decode((await reader.read()).value)).toBe(
    'live chunk',
  );
  await reader.cancel();
});

it('preserves backend errors instead of trying direct playback', async () => {
  fetchMock.mockResolvedValue(
    new Response('Target host is not allowed', { status: 400 }),
  );
  const response = await handleStreamProxyRequest(
    new Request(
      'https://app.example/stream-proxy?url=http://127.0.0.1/private',
    ),
  );
  expect(response.status).toBe(400);
  expect(await response.text()).toBe('Target host is not allowed');
  expect(fetchMock).toHaveBeenCalledOnce();
});

it('never follows redirects from the authenticated backend', async () => {
  fetchMock.mockResolvedValue(
    new Response(null, {
      status: 302,
      headers: { Location: 'https://untrusted.example' },
    }),
  );
  const response = await handleStreamProxyRequest(
    new Request(
      'https://app.example/stream-proxy?url=https://cdn.example/live',
    ),
  );
  expect(response.status).toBe(502);
  expect(response.headers.has('Location')).toBe(false);
  expect(fetchMock).toHaveBeenCalledOnce();
});

it('reports cancellation without leaking backend fetch errors', async () => {
  const controller = new AbortController();
  controller.abort();
  fetchMock.mockRejectedValue(
    new Error('https://backend.example/?token=secret'),
  );
  const response = await handleStreamProxyRequest(
    new Request(
      'https://app.example/stream-proxy?url=https://cdn.example/live',
      { signal: controller.signal },
    ),
  );
  expect(response.status).toBe(499);
  expect(await response.text()).toBe('');
  const failure = await handleStreamProxyRequest(
    new Request(
      'https://app.example/stream-proxy?url=https://cdn.example/live',
    ),
  );
  expect(failure.status).toBe(502);
  expect(await failure.text()).not.toContain('secret');
});
