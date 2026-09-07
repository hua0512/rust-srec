import {
  FileVideo,
  Upload,
  Image as ImageIcon,
  Cloud,
  CloudUpload,
  Terminal,
  Copy,
  Scissors,
  Archive,
  Trash,
  Tag,
  Workflow,
  Globe,
  Video,
  Radio,
  Film,
  Camera,
  Flame,
  Type,
  Tv,
} from 'lucide-react';
import {
  SiBilibili,
  SiSinaweibo,
  SiTiktok,
  SiTwitch,
  SiXiaohongshu,
  SiYoutube,
} from '@icons-pack/react-simple-icons';
import React from 'react';

/**
 * Keyed by processor name, preset category, and the leading segment of the
 * built-in preset names (`compress_hq`, `audio_mp3`, `nvenc_av1_fast`, ...),
 * which is what a workflow step stores when it references a preset.
 */
export const STEP_ICONS: Record<string, React.ElementType> = {
  remux: FileVideo,
  thumbnail: ImageIcon,
  upload: Upload,
  rclone: Cloud,
  baidupcs: CloudUpload,
  execute: Terminal,
  copy_move: Copy,
  copy: Copy,
  move: Copy,
  audio_extract: Scissors,
  audio: Scissors,
  compression: Archive,
  compress: Archive,
  nvenc: Archive,
  archive: Archive,
  delete: Trash,
  metadata: Tag,
  add_metadata: Tag,
  danmaku_factory: Type,
  danmu: Type,
  ass_burnin: Flame,
  subtitle: Flame,
  custom: Terminal,
};

/**
 * Colour variants for a pipeline step.
 *
 * Tailwind generates utilities by scanning source text, so each variant holds
 * complete class strings. Deriving one utility from another at render time
 * (turning `bg-blue-500` into `border-blue-500`, for example) yields classes
 * that never reach the stylesheet.
 */
export type StepColorVariants = {
  /** Gradient surface used by the workflow editor's step rows. */
  gradient: string;
  /** Solid tinted chip used by compact step badges and preset tiles. */
  badge: string;
};

export const STEP_COLORS: Record<string, StepColorVariants> = {
  remux: {
    gradient: 'from-blue-500/10 to-blue-500/5 text-blue-500 border-blue-500/20',
    badge: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
  },
  thumbnail: {
    gradient:
      'from-purple-500/10 to-purple-500/5 text-purple-500 border-purple-500/20',
    badge: 'bg-purple-500/10 text-purple-500 border-purple-500/20',
  },
  upload: {
    gradient:
      'from-green-500/10 to-green-500/5 text-green-500 border-green-500/20',
    badge: 'bg-green-500/10 text-green-500 border-green-500/20',
  },
  rclone: {
    gradient:
      'from-emerald-500/10 to-emerald-500/5 text-emerald-500 border-emerald-500/20',
    badge: 'bg-emerald-500/10 text-emerald-500 border-emerald-500/20',
  },
  baidupcs: {
    gradient: 'from-sky-500/10 to-sky-500/5 text-sky-500 border-sky-500/20',
    badge: 'bg-sky-500/10 text-sky-500 border-sky-500/20',
  },
  execute: {
    gradient: 'from-gray-500/10 to-gray-500/5 text-gray-500 border-gray-500/20',
    badge: 'bg-gray-500/10 text-gray-500 border-gray-500/20',
  },
  audio_extract: {
    gradient: 'from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20',
    badge: 'bg-pink-500/10 text-pink-500 border-pink-500/20',
  },
  audio: {
    gradient: 'from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20',
    badge: 'bg-pink-500/10 text-pink-500 border-pink-500/20',
  },
  compression: {
    gradient:
      'from-orange-500/10 to-orange-500/5 text-orange-500 border-orange-500/20',
    badge: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
  },
  compress: {
    gradient:
      'from-orange-500/10 to-orange-500/5 text-orange-500 border-orange-500/20',
    badge: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
  },
  nvenc: {
    gradient:
      'from-orange-500/10 to-orange-500/5 text-orange-500 border-orange-500/20',
    badge: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
  },
  delete: {
    gradient: 'from-red-500/10 to-red-500/5 text-red-500 border-red-500/20',
    badge: 'bg-red-500/10 text-red-500 border-red-500/20',
  },
  cleanup: {
    gradient: 'from-red-500/10 to-red-500/5 text-red-500 border-red-500/20',
    badge: 'bg-red-500/10 text-red-500 border-red-500/20',
  },
  metadata: {
    gradient: 'from-cyan-500/10 to-cyan-500/5 text-cyan-500 border-cyan-500/20',
    badge: 'bg-cyan-500/10 text-cyan-500 border-cyan-500/20',
  },
  add_metadata: {
    gradient: 'from-cyan-500/10 to-cyan-500/5 text-cyan-500 border-cyan-500/20',
    badge: 'bg-cyan-500/10 text-cyan-500 border-cyan-500/20',
  },
  copy_move: {
    gradient:
      'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
    badge: 'bg-amber-500/10 text-amber-500 border-amber-500/20',
  },
  copy: {
    gradient:
      'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
    badge: 'bg-amber-500/10 text-amber-500 border-amber-500/20',
  },
  move: {
    gradient:
      'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
    badge: 'bg-amber-500/10 text-amber-500 border-amber-500/20',
  },
  file_ops: {
    gradient:
      'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
    badge: 'bg-amber-500/10 text-amber-500 border-amber-500/20',
  },
  archive: {
    gradient:
      'from-yellow-500/10 to-yellow-500/5 text-yellow-500 border-yellow-500/20',
    badge: 'bg-yellow-500/10 text-yellow-500 border-yellow-500/20',
  },
  danmaku_factory: {
    gradient:
      'from-indigo-500/10 to-indigo-500/5 text-indigo-500 border-indigo-500/20',
    badge: 'bg-indigo-500/10 text-indigo-500 border-indigo-500/20',
  },
  danmu: {
    gradient:
      'from-indigo-500/10 to-indigo-500/5 text-indigo-500 border-indigo-500/20',
    badge: 'bg-indigo-500/10 text-indigo-500 border-indigo-500/20',
  },
  ass_burnin: {
    gradient:
      'from-orange-600/10 to-orange-600/5 text-orange-600 border-orange-600/20',
    badge: 'bg-orange-600/10 text-orange-600 border-orange-600/20',
  },
  subtitle: {
    gradient:
      'from-orange-600/10 to-orange-600/5 text-orange-600 border-orange-600/20',
    badge: 'bg-orange-600/10 text-orange-600 border-orange-600/20',
  },
  custom: {
    gradient:
      'from-slate-500/10 to-slate-500/5 text-slate-500 border-slate-500/20',
    badge: 'bg-slate-500/10 text-slate-500 border-slate-500/20',
  },
};

export const DEFAULT_STEP_COLOR: StepColorVariants = {
  gradient: 'from-primary/10 to-primary/5 text-primary border-primary/20',
  badge: 'bg-primary/10 text-primary border-primary/20',
};

/**
 * Resolves a step's colours from its processor name, falling back to its
 * preset category and then to a matching preset-name prefix.
 */
export function getStepColorVariants(
  processor: string,
  category?: string,
): StepColorVariants {
  if (STEP_COLORS[processor]) return STEP_COLORS[processor];
  if (category && STEP_COLORS[category]) return STEP_COLORS[category];
  for (const [key, variants] of Object.entries(STEP_COLORS)) {
    if (processor.startsWith(key)) return variants;
  }
  return DEFAULT_STEP_COLOR;
}

export function getStepColor(processor: string, category?: string): string {
  return getStepColorVariants(processor, category).gradient;
}

export function getStepBadgeColor(
  processor: string,
  category?: string,
): string {
  return getStepColorVariants(processor, category).badge;
}

export function getStepIcon(processor: string): React.ElementType {
  if (STEP_ICONS[processor]) return STEP_ICONS[processor];
  for (const [key, Icon] of Object.entries(STEP_ICONS)) {
    if (processor.startsWith(key)) return Icon;
  }
  return Workflow;
}

// Platform Constants
export const PLATFORM_ICONS: Record<string, React.ElementType> = {
  bilibili: SiBilibili,
  douyin: SiTiktok, // Douyin is the Chinese TikTok; Simple Icons has no separate douyin slug
  tiktok: SiTiktok,
  douyu: Radio,
  huya: Video,
  twitch: SiTwitch,
  youtube: SiYoutube,
  acfun: Film,
  pandatv: Camera,
  picarto: ImageIcon,
  redbook: SiXiaohongshu, // Xiaohongshu
  twitcasting: Radio,
  weibo: SiSinaweibo,
  soop: Tv,
  bigo: Radio,
};

export const PLATFORM_COLORS: Record<string, string> = {
  bilibili: 'bg-pink-500/10 text-pink-500 border-pink-500/20',
  douyin: 'bg-cyan-500/10 text-cyan-500 border-cyan-500/20',
  douyu: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
  huya: 'bg-yellow-500/10 text-yellow-500 border-yellow-500/20',
  twitch: 'bg-purple-500/10 text-purple-500 border-purple-500/20',
  youtube: 'bg-red-500/10 text-red-500 border-red-500/20',
  tiktok: 'bg-slate-500/10 text-slate-500 border-slate-500/20',
  acfun: 'bg-red-500/10 text-red-500 border-red-500/20',
  pandatv: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
  picarto: 'bg-green-500/10 text-green-500 border-green-500/20',
  redbook: 'bg-rose-500/10 text-rose-500 border-rose-500/20',
  twitcasting: 'bg-indigo-500/10 text-indigo-500 border-indigo-500/20',
  weibo: 'bg-amber-500/10 text-amber-500 border-amber-500/20',
  soop: 'bg-emerald-500/10 text-emerald-500 border-emerald-500/20',
  bigo: 'bg-sky-500/10 text-sky-500 border-sky-500/20',
};

export function getPlatformIcon(platform: string): React.ElementType {
  const key = platform.toLowerCase();
  return PLATFORM_ICONS[key] || Globe;
}

export function getPlatformColor(platform: string): string {
  const key = platform.toLowerCase();
  return PLATFORM_COLORS[key] || 'bg-primary/10 text-primary border-primary/20';
}

// Metadata placeholder tokens supported by processor destination templates
// (rclone destination_root, copy/move destination). Kept out of lingui
// <Trans> messages as a bound value: literal `{...}` tokens inlined into a
// message become unbound ICU placeholders and render as empty strings.
export const PLACEHOLDER_TOKENS =
  '{platform}, {streamer}, {title}, {streamer_id}, {session_id}';
