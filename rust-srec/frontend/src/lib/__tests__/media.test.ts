import {
  isPlayable,
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
