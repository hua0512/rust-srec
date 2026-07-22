const ensureValidTokenMock = vi.hoisted(() => vi.fn());

vi.mock('../tokenRefresh', () => ({
  ensureValidToken: ensureValidTokenMock,
}));

import {
  handleStreamProxyRequest,
  rewriteHlsManifest,
  validateProxyTarget,
  type AddressResolver,
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

describe('validateProxyTarget', () => {
  const publicResolver: AddressResolver = async () => [
    { address: '203.0.114.10', family: 4 },
  ];

  it('allows public http targets', async () => {
    await expect(
      validateProxyTarget('https://media.example/live.m3u8', publicResolver),
    ).resolves.toHaveProperty('hostname', 'media.example');
  });

  it.each([
    'http://127.0.0.1/stream',
    'http://10.0.0.1/stream',
    'http://169.254.169.254/latest/meta-data',
    'http://[::1]/stream',
    'http://[fd00::1]/stream',
  ])('rejects non-public literal address %s', async (target) => {
    await expect(validateProxyTarget(target, publicResolver)).rejects.toThrow(
      'Target host is not allowed',
    );
  });

  it('rejects hostnames with any private DNS result', async () => {
    const resolver: AddressResolver = async () => [
      { address: '203.0.114.10', family: 4 },
      { address: '192.168.1.2', family: 4 },
    ];

    await expect(
      validateProxyTarget('https://media.example/live.m3u8', resolver),
    ).rejects.toThrow('Target host is not allowed');
  });

  it.each(['file:///etc/passwd', 'https://user:pass@media.example/stream'])(
    'rejects unsafe target %s',
    async (target) => {
      await expect(
        validateProxyTarget(target, publicResolver),
      ).rejects.toThrow();
    },
  );
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
