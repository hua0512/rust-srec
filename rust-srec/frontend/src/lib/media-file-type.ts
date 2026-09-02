import { msg } from '@lingui/core/macro';
import type { MessageDescriptor } from '@lingui/core';
import {
  File,
  FileAudio,
  FileVideo,
  Image,
  MessageSquare,
  type LucideIcon,
} from 'lucide-react';

/**
 * How to present one `media_outputs.file_type` value.
 *
 * `tile` is a gradient/text/border triple for the icon tile behind
 * `bg-gradient-to-br`; `badge` reuses it for the inline type chip. Both follow
 * the token vocabulary of `STEP_COLORS` in `components/pipeline/constants.tsx`.
 */
export interface MediaFileTypeMeta {
  label: MessageDescriptor;
  icon: LucideIcon;
  tile: string;
}

/**
 * Presentation for each variant of `MediaFileType` in
 * `rust-srec/src/domain/session/entity.rs`.
 *
 * These are the only values `MediaOutput.format` ever holds — the API field is
 * named `format` but carries `media_outputs.file_type`, not a container format.
 * Deriving `.mp4` vs `.flv` needs `getMediaExtension` in `lib/media.ts` on the
 * file path instead.
 */
const MEDIA_FILE_TYPES: Record<string, MediaFileTypeMeta> = {
  VIDEO: {
    label: msg`Video`,
    icon: FileVideo,
    tile: 'from-blue-500/10 to-blue-500/5 text-blue-500 border-blue-500/20',
  },
  AUDIO: {
    label: msg`Audio`,
    icon: FileAudio,
    tile: 'from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20',
  },
  THUMBNAIL: {
    label: msg`Thumbnail`,
    icon: Image,
    tile: 'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
  },
  DANMU_XML: {
    label: msg`Danmaku`,
    icon: MessageSquare,
    tile: 'from-violet-500/10 to-violet-500/5 text-violet-500 border-violet-500/20',
  },
};

/** File types in the order the type filter and any legend should list them. */
export const MEDIA_FILE_TYPE_ORDER = Object.keys(MEDIA_FILE_TYPES);

/**
 * Resolve the label with `i18n._`.
 *
 * A type added to the backend but not yet listed above still has to render as
 * something, so it falls back to a neutral icon and the raw value with its
 * underscores removed. A descriptor with no catalog entry resolves to its own
 * id, which is that humanized name.
 */
export function getMediaFileTypeMeta(fileType: string): MediaFileTypeMeta {
  return (
    MEDIA_FILE_TYPES[fileType] ?? {
      label: { id: fileType.replace(/_/g, ' ') },
      icon: File,
      tile: 'from-slate-500/10 to-slate-500/5 text-slate-500 border-slate-500/20',
    }
  );
}
