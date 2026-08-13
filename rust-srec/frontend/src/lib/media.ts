import { getMediaUrl } from '@/lib/url';

export type PlayerMediaType =
  | 'hls'
  | 'flv'
  | 'mpegts'
  | 'mp4'
  | 'mkv'
  | 'audio'
  | 'native'
  | 'auto';

const PLAYABLE_EXTENSIONS = new Set([
  'mp4',
  'webm',
  'ogg',
  'mp3',
  'wav',
  'mkv',
  'flv',
  'ts',
  'm3u8',
]);

const MEDIA_TYPE_ALIASES: Readonly<Record<string, PlayerMediaType>> = {
  hls: 'hls',
  m3u8: 'hls',
  flv: 'flv',
  'http-flv': 'flv',
  ts: 'mpegts',
  m2ts: 'mpegts',
  mpegts: 'mpegts',
  'mpeg-ts': 'mpegts',
  mp4: 'mp4',
  fmp4: 'mp4',
  m4v: 'mp4',
  mov: 'mp4',
  mkv: 'mkv',
  matroska: 'mkv',
  mp3: 'audio',
  wav: 'audio',
  ogg: 'audio',
  aac: 'audio',
  m4a: 'audio',
  flac: 'audio',
  webm: 'native',
  auto: 'auto',
};

function getMediaExtension(value: string): string | undefined {
  const path = value.split(/[?#]/, 1)[0]?.replaceAll('\\', '/');
  const fileName = path?.split('/').pop();
  if (!fileName) return undefined;

  const extensionIndex = fileName.lastIndexOf('.');
  if (extensionIndex <= 0 || extensionIndex === fileName.length - 1) {
    return undefined;
  }

  return fileName.slice(extensionIndex + 1).toLowerCase();
}

export function normalizePlayerMediaType(
  value: string | null | undefined,
): PlayerMediaType | undefined {
  if (!value) return undefined;
  return MEDIA_TYPE_ALIASES[value.trim().toLowerCase().replace(/^\./, '')];
}

export function resolvePlayerMediaType(
  explicitType: string | null | undefined,
  ...sources: Array<string | null | undefined>
): PlayerMediaType {
  const normalizedType = normalizePlayerMediaType(explicitType);
  if (normalizedType) return normalizedType;

  for (const source of sources) {
    if (!source) continue;
    const extension = getMediaExtension(source);
    const detectedType = normalizePlayerMediaType(extension);
    if (detectedType) return detectedType;
  }

  return 'auto';
}

export function isPlayable(output: {
  format: string;
  file_path: string;
}): boolean {
  // Filter out thumbnails and danmu files
  if (output.format === 'THUMBNAIL' || output.format === 'DANMU_XML')
    return false;

  // Whitelist supported extensions
  const extension = getMediaExtension(output.file_path);

  return extension !== undefined && PLAYABLE_EXTENSIONS.has(extension);
}

// Structural subset of MediaOutput needed to pick a media source.
export interface OutputMediaSource {
  id: string;
  remote_url?: string | null;
  local_available: boolean;
}

/**
 * URL an output should be played/downloaded from: the authenticated backend
 * endpoint while the local file exists, otherwise the cloud copy recorded at
 * upload time (remote_url). Falls back to the backend endpoint when no cloud
 * copy exists — it 404s exactly like before this field existed.
 *
 * getMediaUrl passes absolute http(s) URLs through untouched and never
 * appends the auth token to them.
 */
export function getOutputMediaUrl(
  output: OutputMediaSource,
  token?: string,
): string | null {
  if (!output.local_available && output.remote_url) {
    return getMediaUrl(output.remote_url, token);
  }
  return getMediaUrl(`/api/media/${output.id}/content`, token);
}

/**
 * True when the output can only be served from its cloud copy — the local
 * file is gone and remote_url is the effective source. Downloads must then
 * navigate to the URL directly instead of fetching with the auth header
 * (cross-origin fetch would need CORS and would leak the JWT).
 */
export function isRemoteOnly(output: OutputMediaSource): boolean {
  return !output.local_available && Boolean(output.remote_url);
}
