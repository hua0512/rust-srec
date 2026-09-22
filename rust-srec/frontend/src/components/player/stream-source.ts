import { resolvePlayerMediaType } from '@/lib/media';

export interface StreamOption {
  url: string;
  data?: unknown;
  quality?: string;
  cdn?: string;
  format?: string;
  bitrate?: number;
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

export function selectRefreshedStream(
  mediaInfo: unknown,
  selected: StreamOption,
): StreamOption | undefined {
  const streams = extractStreams(mediaInfo);
  return (
    streams.find(
      (stream) =>
        stream.quality === selected.quality &&
        stream.cdn === selected.cdn &&
        stream.format === selected.format,
    ) ??
    streams.find(
      (stream) =>
        stream.quality === selected.quality &&
        stream.format === selected.format,
    ) ??
    streams[0]
  );
}
