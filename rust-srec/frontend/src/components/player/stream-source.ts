import { resolvePlayerMediaType } from '@/lib/media';

export interface StreamOption {
  url: string;
  data?: unknown;
  quality?: string;
  cdn?: string;
  format?: string;
  bitrate?: number;
  codec?: string;
  container?: string;
  fps?: number;
  headers?: Record<string, string>;
  extras?: Record<string, string>;
}

// Helper function to extract all stream options from media_info
export function extractStreams(mediaInfo: any): StreamOption[] {
  const streams: StreamOption[] = [];

  if (!mediaInfo) return streams;
  if (typeof mediaInfo === 'string')
    return [
      { url: mediaInfo, format: resolvePlayerMediaType(undefined, mediaInfo) },
    ];

  // Handle different possible structures
  if (Array.isArray(mediaInfo.streams)) {
    mediaInfo.streams.forEach((stream: any) => {
      const extras = stringifyValues({ ...mediaInfo.extras, ...stream.extras });
      streams.push({
        url: stream.url || stream.src || '',
        data: stream,
        quality: stream.quality || stream.resolution || 'unknown',
        cdn: stream.cdn || stream.server || extras.cdn,
        format:
          stream.format ||
          stream.stream_format ||
          resolvePlayerMediaType(undefined, stream.url),
        bitrate: stream.bitrate || stream.bandwidth,
        codec: typeof stream.codec === 'string' ? stream.codec : undefined,
        container:
          typeof stream.media_format === 'string'
            ? stream.media_format
            : undefined,
        fps:
          typeof stream.fps === 'number' && Number.isFinite(stream.fps)
            ? stream.fps
            : undefined,
        headers: { ...mediaInfo.headers, ...stream.headers },
        extras,
      });
    });
  } else if (mediaInfo.url) {
    // Single stream
    const extras = stringifyValues(mediaInfo.extras || {});
    streams.push({
      url: mediaInfo.url,
      quality: mediaInfo.quality || 'default',
      cdn: mediaInfo.cdn || extras.cdn,
      format:
        mediaInfo.format ||
        mediaInfo.stream_format ||
        resolvePlayerMediaType(undefined, mediaInfo.url),
      bitrate: mediaInfo.bitrate,
      codec: typeof mediaInfo.codec === 'string' ? mediaInfo.codec : undefined,
      fps:
        typeof mediaInfo.fps === 'number' && Number.isFinite(mediaInfo.fps)
          ? mediaInfo.fps
          : undefined,
      headers: mediaInfo.headers || {},
      extras,
    });
  }

  return streams.filter((stream) => {
    const raw = stream.data;
    // Some extractors return selectable variants whose URL is filled by /parse/resolve.
    return (
      stream.url ||
      (raw &&
        typeof raw === 'object' &&
        'stream_format' in raw &&
        'media_format' in raw)
    );
  });
}

function stringifyValues(obj: Record<string, any>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key in obj) {
    if (obj[key] !== undefined && obj[key] !== null) {
      result[key] = String(obj[key]);
    }
  }
  return result;
}

// Selection hierarchy, outermost first. Each level only offers the values
// available under the levels above it, mirroring how extractors nest streams.
export const streamLevels = ['format', 'cdn', 'quality', 'variant'] as const;
export type StreamLevel = (typeof streamLevels)[number];

export function streamLevelKey(stream: StreamOption, level: StreamLevel) {
  switch (level) {
    case 'format':
      return stream.format?.toLowerCase() ?? '';
    case 'cdn':
      return stream.cdn ?? '';
    case 'quality':
      return stream.quality ?? '';
    case 'variant':
      return `${stream.codec?.toLowerCase() ?? ''}/${stream.container?.toLowerCase() ?? ''}`;
  }
}

/** Narrows `streams` level by level, skipping a level when nothing matches it. */
function narrowTo(
  streams: StreamOption[],
  target: StreamOption,
  levels: readonly StreamLevel[],
) {
  return levels.reduce((candidates, level) => {
    const key = streamLevelKey(target, level);
    const matching = candidates.filter(
      (stream) => streamLevelKey(stream, level) === key,
    );
    return matching.length > 0 ? matching : candidates;
  }, streams);
}

/**
 * Distinct values for `level` among streams sharing the selection's outer
 * levels, in source order, each paired with its first stream.
 */
export function streamLevelOptions(
  streams: StreamOption[],
  selected: StreamOption,
  level: StreamLevel,
) {
  const outer = streamLevels.slice(0, streamLevels.indexOf(level));
  const options = new Map<string, StreamOption>();
  for (const stream of streams) {
    if (
      outer.every(
        (l) => streamLevelKey(stream, l) === streamLevelKey(selected, l),
      )
    ) {
      const key = streamLevelKey(stream, level);
      if (!options.has(key)) options.set(key, stream);
    }
  }
  return [...options].map(([value, stream]) => ({ value, stream }));
}

/**
 * Picks the stream for `value` at `level`, keeping the selection's outer
 * levels and, where the new branch offers them, its inner levels too.
 */
export function selectStreamLevel(
  streams: StreamOption[],
  selected: StreamOption,
  level: StreamLevel,
  value: string,
): StreamOption | undefined {
  const index = streamLevels.indexOf(level);
  const outer = streamLevels.slice(0, index);
  const candidates = streams.filter(
    (stream) =>
      streamLevelKey(stream, level) === value &&
      outer.every(
        (l) => streamLevelKey(stream, l) === streamLevelKey(selected, l),
      ),
  );
  return narrowTo(candidates, selected, streamLevels.slice(index + 1))[0];
}

export function selectRefreshedStream(
  mediaInfo: unknown,
  selected: StreamOption,
): StreamOption | undefined {
  // A matching format and quality matter more than keeping the same CDN.
  return narrowTo(extractStreams(mediaInfo), selected, [
    'format',
    'quality',
    'cdn',
    'variant',
  ])[0];
}
