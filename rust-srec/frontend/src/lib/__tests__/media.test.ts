import {
  getOutputMediaUrl,
  isPlayable,
  isRemoteOnly,
  normalizePlayerMediaType,
  resolvePlayerMediaType,
} from '../media';

describe('player media type resolution', () => {
  it('normalizes backend format aliases', () => {
    expect(normalizePlayerMediaType('HLS')).toBe('hls');
    expect(normalizePlayerMediaType('http-flv')).toBe('flv');
    expect(normalizePlayerMediaType('mpeg-ts')).toBe('mpegts');
    expect(normalizePlayerMediaType('fmp4')).toBe('mp4');
  });

  it('prefers an explicit format over URL and title fallbacks', () => {
    expect(
      resolvePlayerMediaType(
        'hls',
        'https://example.com/recording.mp4',
        'recording.flv',
      ),
    ).toBe('hls');
  });

  it('uses the final path extension instead of substring matches', () => {
    expect(resolvePlayerMediaType(undefined, 'recording.ts.mp4')).toBe('mp4');
    expect(resolvePlayerMediaType(undefined, 'recording.mp4.ts')).toBe(
      'mpegts',
    );
  });

  it('ignores query strings and fragments during fallback detection', () => {
    expect(
      resolvePlayerMediaType(
        undefined,
        '/api/media/id/content?token=header.ts.signature',
        'recording.mp4#chapter.ts',
      ),
    ).toBe('mp4');
  });

  it('returns auto when no supported type is known', () => {
    expect(resolvePlayerMediaType('unknown', '/api/media/id/content')).toBe(
      'auto',
    );
  });
});

describe('isPlayable', () => {
  it('accepts supported extensions and ignores query strings', () => {
    expect(
      isPlayable({ format: 'VIDEO', file_path: '/recordings/video.mp4?v=1' }),
    ).toBe(true);
  });

  it('rejects non-media outputs and misleading suffixes', () => {
    expect(
      isPlayable({ format: 'THUMBNAIL', file_path: '/recordings/image.mp4' }),
    ).toBe(false);
    expect(
      isPlayable({ format: 'VIDEO', file_path: '/recordings/video.mp4.tmp' }),
    ).toBe(false);
  });
});

describe('getOutputMediaUrl', () => {
  it('uses the backend endpoint with token while the local file exists', () => {
    expect(
      getOutputMediaUrl(
        { id: 'abc', remote_url: 'https://cdn.example.com/a.mp4', local_available: true },
        'jwt',
      ),
    ).toBe('/api/media/abc/content?token=jwt');
  });

  it('uses the cloud copy without token when the local file is gone', () => {
    expect(
      getOutputMediaUrl(
        { id: 'abc', remote_url: 'https://cdn.example.com/a.mp4', local_available: false },
        'jwt',
      ),
    ).toBe('https://cdn.example.com/a.mp4');
  });

  it('falls back to the backend endpoint when no cloud copy exists', () => {
    expect(
      getOutputMediaUrl({ id: 'abc', remote_url: null, local_available: false }),
    ).toBe('/api/media/abc/content');
  });
});

describe('isRemoteOnly', () => {
  it('is true only when the local file is gone and a cloud copy exists', () => {
    expect(
      isRemoteOnly({ id: 'a', remote_url: 'https://x/a.mp4', local_available: false }),
    ).toBe(true);
    expect(
      isRemoteOnly({ id: 'a', remote_url: 'https://x/a.mp4', local_available: true }),
    ).toBe(false);
    expect(isRemoteOnly({ id: 'a', remote_url: null, local_available: false })).toBe(
      false,
    );
  });
});
