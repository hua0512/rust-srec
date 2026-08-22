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
