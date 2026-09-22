import {
  extractStreams,
  selectRefreshedStream,
  selectStreamLevel,
  streamLevelOptions,
} from '../stream-source';
import { classifyPlaybackError } from '../playback-state';

describe('stream selection across refreshes', () => {
  const hls = {
    url: 'https://media.example/new.m3u8',
    quality: '1080p',
    stream_format: 'hls',
    extras: { cdn: 'primary' },
  };
  const flv = {
    ...hls,
    url: 'https://media.example/new.flv',
    stream_format: 'flv',
  };
  const media = {
    streams: [flv, hls],
    headers: { Referer: 'https://source.example' },
  };

  it('preserves format, quality and CDN even when stream order and signed URLs change', () => {
    const selected = selectRefreshedStream(media, {
      url: 'https://media.example/expired.m3u8',
      quality: '1080p',
      format: 'hls',
      cdn: 'primary',
    });
    expect(selected?.url).toBe(hls.url);
    expect(selected?.data).toBe(hls);
    expect(selected?.headers).toEqual(media.headers);
  });

  it('falls back to a playable option and excludes empty URLs', () => {
    const options = { streams: [{ ...hls, url: '' }, flv] };
    expect(selectRefreshedStream(options, { url: 'expired' })?.url).toBe(
      flv.url,
    );
    expect(
      selectRefreshedStream({ streams: [] }, { url: 'expired' }),
    ).toBeUndefined();
  });

  it('handles single URLs and per-source headers', () => {
    expect(extractStreams('https://media.example/video.mp4')[0]?.format).toBe(
      'mp4',
    );
    expect(
      extractStreams({
        headers: { Referer: 'global' },
        streams: [{ ...hls, headers: { Referer: 'source' } }],
      })[0]?.headers,
    ).toEqual({ Referer: 'source' });
  });

  it('keeps deferred backend variants so they can be resolved after selection', () => {
    const deferred = { ...hls, url: '', media_format: 'mp4' };
    const options = { streams: [deferred] };
    const selected = extractStreams(options)[0]!;
    expect(selected.data).toBe(deferred);
    expect(selectRefreshedStream(options, selected)?.data).toBe(deferred);
  });
});

describe('multi-CDN, multi-quality selection', () => {
  // Shaped like Bilibili: each protocol lists codecs and containers, each
  // served from several CDNs at several qualities.
  const media = {
    streams: [
      ['flv', 'flv', 'avc', 'cn-gotcha01', '原画'],
      ['flv', 'flv', 'avc', 'cn-gotcha01', '蓝光'],
      ['flv', 'flv', 'avc', 'cn-hk-eq', '原画'],
      ['hls', 'ts', 'avc', 'cn-gotcha01', '原画'],
      ['hls', 'fmp4', 'avc', 'cn-gotcha01', '原画'],
      ['hls', 'fmp4', 'hevc', 'cn-gotcha01', '原画'],
      ['hls', 'fmp4', 'hevc', 'cn-gotcha01', '蓝光'],
      ['hls', 'fmp4', 'hevc', 'cn-hk-eq', '蓝光'],
    ].map(([stream_format, media_format, codec, cdn, quality], index) => ({
      url: `https://media.example/${index}`,
      stream_format,
      media_format,
      codec,
      quality,
      extras: { cdn },
    })),
  };
  const streams = extractStreams(media);
  const values = (
    selected: (typeof streams)[number],
    level: Parameters<typeof streamLevelOptions>[2],
  ) => streamLevelOptions(streams, selected, level).map(({ value }) => value);

  it('offers each level only within the selected outer levels', () => {
    const hevc = streams[5];
    expect(values(hevc, 'format')).toEqual(['flv', 'hls']);
    expect(values(hevc, 'cdn')).toEqual(['cn-gotcha01', 'cn-hk-eq']);
    expect(values(hevc, 'quality')).toEqual(['原画', '蓝光']);
    // Same format, CDN and quality still leave codec/container variants.
    expect(values(hevc, 'variant')).toEqual([
      'avc/ts',
      'avc/fmp4',
      'hevc/fmp4',
    ]);
    expect(values(streams[0], 'variant')).toEqual(['avc/flv']);
  });

  it('keeps the quality and codec when switching CDN or format', () => {
    expect(selectStreamLevel(streams, streams[6], 'cdn', 'cn-hk-eq')).toBe(
      streams[7],
    );
    expect(selectStreamLevel(streams, streams[1], 'format', 'hls')).toBe(
      streams[6],
    );
    // A CDN without the current quality falls back to its first stream.
    expect(selectStreamLevel(streams, streams[1], 'cdn', 'cn-hk-eq')).toBe(
      streams[2],
    );
  });

  it('keeps the codec and container across refreshes', () => {
    expect(selectRefreshedStream(media, { ...streams[5], url: 'old' })).toEqual(
      streams[5],
    );
  });
});

describe('actionable playback errors', () => {
  it.each([
    ['networkError', 403, true, 'authentication'],
    ['networkError', 410, false, 'unavailable'],
    ['networkError', 400, true, 'rejected'],
    ['networkError', 502, true, 'upstream'],
    ['networkError', 502, false, 'network'],
    ['mediaError', undefined, false, 'media'],
    ['NotSupportedError', undefined, false, 'unsupported'],
  ] as const)(
    'classifies %s / %s (proxy: %s)',
    (type, status, proxy, expected) => {
      expect(classifyPlaybackError(type, status, proxy)).toBe(expected);
    },
  );
});
