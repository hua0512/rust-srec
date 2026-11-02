import {
  findDanmuOutput,
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

  it('recognises DASH so it is never handed to native playback', () => {
    expect(normalizePlayerMediaType('DASH')).toBe('dash');
    expect(normalizePlayerMediaType('.mpd')).toBe('dash');
    expect(
      resolvePlayerMediaType(
        undefined,
        'https://cdn.example/live/manifest.mpd?t=1',
      ),
    ).toBe('dash');
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

describe('findDanmuOutput', () => {
  const xml = (file_path: string) => ({ format: 'DANMU_XML', file_path });
  const video = { format: 'VIDEO', file_path: '/rec/Alice/seg_1.mp4' };

  it('pairs the XML recorded beside the segment, not a prefix match', () => {
    const outputs = [
      xml('/rec/Alice/seg_10.xml'),
      { format: 'VIDEO', file_path: '/rec/Alice/seg_1.flv' },
      xml('/rec/Alice/seg_1.xml'),
    ];

    expect(findDanmuOutput(video, outputs)).toBe(outputs[2]);
  });

  it('accepts a moved file only when its name is unambiguous', () => {
    const moved = xml('C:\\archive\\seg_1.xml');
    expect(findDanmuOutput(video, [moved])).toBe(moved);
    expect(
      findDanmuOutput(video, [moved, xml('/other/seg_1.xml')]),
    ).toBeUndefined();
  });
});
