import { extractStreams, selectRefreshedStream } from '../stream-source';
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
