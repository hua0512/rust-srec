const ensureValidTokenMock = vi.hoisted(() => vi.fn());

vi.mock('../tokenRefresh', () => ({
  ensureValidToken: ensureValidTokenMock,
}));

import {
  fetchWithValidatedRedirects,
  handleStreamProxyRequest,
  parseCustomHeaders,
  rewriteHlsManifest,
  validateProxyTarget,
  type AddressResolver,
  type UpstreamFetch,
} from '../stream-proxy';

describe('rewriteHlsManifest', () => {
  it('proxies playlists, segments, keys, maps, and low-latency parts', () => {
    const manifest = [
      '#EXTM3U',
      '#EXT-X-MEDIA:TYPE=AUDIO,URI="audio/index.m3u8"',
      '#EXT-X-STREAM-INF:BANDWIDTH=1000',
      'video/index.m3u8',
      '#EXT-X-KEY:METHOD=AES-128,URI="../key.bin"',
      '#EXT-X-MAP:URI = "init.mp4"',
      '#EXT-X-PART:DURATION=0.333,URI="part.ts"',
      'segment.ts',
    ].join('\r\n');

    const rewritten = rewriteHlsManifest(
      manifest,
      new URL('https://media.example/live/master.m3u8'),
      { Cookie: 'session=secret' },
    );

    expect(rewritten.match(/\/stream-proxy\?/g)).toHaveLength(6);
    expect(decodeURIComponent(rewritten)).toContain(
      'url=https://media.example/live/audio/index.m3u8',
    );
    expect(decodeURIComponent(rewritten)).toContain(
      'url=https://media.example/key.bin',
    );
    expect(decodeURIComponent(rewritten)).toContain(
      'headers={"Cookie":"session=secret"}',
    );
    expect(rewritten.match(/\r\n/g)).toHaveLength(7);
    expect(rewritten.endsWith('\n')).toBe(false);
  });

  it('leaves non-http key schemes untouched', () => {
    const manifest =
      '#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI="skd://license/key"\nsegment.ts\n';
    const rewritten = rewriteHlsManifest(
      manifest,
      new URL('https://media.example/live/index.m3u8'),
      {},
    );

    expect(rewritten).toContain('URI="skd://license/key"');
    expect(rewritten.match(/\/stream-proxy\?/g)).toHaveLength(1);
  });
});

describe('parseCustomHeaders', () => {
  it('accepts well-formed header maps', () => {
    expect(parseCustomHeaders('{"Referer":"https://source.example/"}')).toEqual(
      {
        Referer: 'https://source.example/',
      },
    );
  });

  it.each([
    ['{"Bad Header":"value"}', 'Invalid header name'],
    ['{"X-Ok":"bad\\nvalue"}', 'Invalid header value'],
  ])('rejects malformed headers with a 400 (%s)', (raw, message) => {
    expect(() => parseCustomHeaders(raw)).toThrow(message);
    try {
      parseCustomHeaders(raw);
    } catch (error) {
      expect(error).toMatchObject({ status: 400 });
    }
  });
});

describe('validateProxyTarget', () => {
  const publicResolver: AddressResolver = async () => [
    { address: '203.0.114.10', family: 4 },
  ];

  it('allows public http targets', async () => {
    await expect(
      validateProxyTarget('https://media.example/live.m3u8', {
        resolver: publicResolver,
      }),
    ).resolves.toHaveProperty('hostname', 'media.example');
  });

  it.each([
    'http://127.0.0.1/stream',
    'http://10.0.0.1/stream',
    'http://169.254.169.254/latest/meta-data',
    'http://[::1]/stream',
    'http://[fd00::1]/stream',
  ])('rejects non-public literal address %s', async (target) => {
    await expect(
      validateProxyTarget(target, { resolver: publicResolver }),
    ).rejects.toThrow('Target host is not allowed');
  });

  it('rejects hostnames with any private DNS result', async () => {
    const resolver: AddressResolver = async () => [
      { address: '203.0.114.10', family: 4 },
      { address: '192.168.1.2', family: 4 },
    ];

    await expect(
      validateProxyTarget('https://media.example/live.m3u8', { resolver }),
    ).rejects.toThrow('Target host is not allowed');
  });

  it.each(['file:///etc/passwd', 'https://user:pass@media.example/stream'])(
    'rejects unsafe target %s',
    async (target) => {
      await expect(
        validateProxyTarget(target, { resolver: publicResolver }),
      ).rejects.toThrow();
    },
  );

  it('allows private and localhost targets when the operator opted in', async () => {
    await expect(
      validateProxyTarget('http://localhost:8080/stream', {
        allowPrivateTargets: true,
      }),
    ).resolves.toHaveProperty('hostname', 'localhost');
    await expect(
      validateProxyTarget('http://192.168.1.20/stream', {
        allowPrivateTargets: true,
      }),
    ).resolves.toHaveProperty('hostname', '192.168.1.20');
    // Scheme and credential checks still apply.
    await expect(
      validateProxyTarget('https://user:pass@192.168.1.20/stream', {
        allowPrivateTargets: true,
      }),
    ).rejects.toThrow('URL credentials are not allowed');
  });
});

describe('fetchWithValidatedRedirects', () => {
  const publicResolver: AddressResolver = async () => [
    { address: '203.0.114.10', family: 4 },
  ];
  const signal = new AbortController().signal;
  const headers = new Headers();

  const redirectResponse = (location: string) =>
    new Response(null, { status: 302, headers: { location } });

  it('rejects redirect hops that land on private addresses', async () => {
    const fetchImpl: UpstreamFetch = vi
      .fn()
      .mockResolvedValueOnce(redirectResponse('http://192.168.1.5/next'));

    await expect(
      fetchWithValidatedRedirects(
        'https://media.example/live.m3u8',
        headers,
        signal,
        { resolver: publicResolver, fetchImpl },
      ),
    ).rejects.toThrow('Target host is not allowed');
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('resolves relative redirect locations against the current target', async () => {
    const fetchImpl: UpstreamFetch = vi
      .fn()
      .mockResolvedValueOnce(redirectResponse('/moved/master.m3u8'))
      .mockResolvedValueOnce(new Response('#EXTM3U', { status: 200 }));

    const { response, finalUrl } = await fetchWithValidatedRedirects(
      'https://media.example/live/master.m3u8',
      headers,
      signal,
      { resolver: publicResolver, fetchImpl },
    );

    expect(response.status).toBe(200);
    expect(finalUrl.toString()).toBe('https://media.example/moved/master.m3u8');
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('caps the number of followed redirects', async () => {
    const fetchImpl: UpstreamFetch = vi
      .fn()
      .mockImplementation(async () =>
        redirectResponse('https://media.example/loop.m3u8'),
      );

    await expect(
      fetchWithValidatedRedirects(
        'https://media.example/live.m3u8',
        headers,
        signal,
        { resolver: publicResolver, fetchImpl },
      ),
    ).rejects.toThrow('Too many upstream redirects');
    expect(fetchImpl).toHaveBeenCalledTimes(6);
  });
});

describe('handleStreamProxyRequest', () => {
  it('requires an authenticated web session before resolving the target', async () => {
    ensureValidTokenMock.mockResolvedValueOnce(null);
    const response = await handleStreamProxyRequest(
      new Request(
        'https://app.example/stream-proxy?url=http%3A%2F%2F127.0.0.1%2Fprivate',
      ),
    );

    expect(response.status).toBe(401);
  });
});
