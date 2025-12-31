import { z } from 'zod';
import { TdlProcessorConfigSchema } from '@/api/schemas/tdl';

// --- Remux Processor ---
export const VideoCodecSchema = z.enum([
  'copy',
  'h264',
  'h265',
  'hevc',
  'vp9',
  'av1',
]);
export const AudioCodecSchema = z.enum(['copy', 'aac', 'mp3', 'opus', 'flac']);
export const PresetSchema = z.enum([
  'ultrafast',
  'superfast',
  'veryfast',
  'faster',
  'fast',
  'medium',
  'slow',
  'slower',
  'veryslow',
]);

export const RemuxConfigSchema = z.object({
  video_codec: VideoCodecSchema.default('copy'),
  audio_codec: AudioCodecSchema.default('copy'),
  format: z.string().optional(),
  video_bitrate: z.string().optional(),
  audio_bitrate: z.string().optional(),
  crf: z.number().min(0).max(51).optional(),
  preset: PresetSchema.optional(),
  resolution: z.string().optional(),
  fps: z.number().optional(),
  start_time: z.number().optional(),
  duration: z.number().optional(),
  end_time: z.number().optional(),
  video_filter: z.string().optional(),
  audio_filter: z.string().optional(),
  hwaccel: z.string().optional(),
  input_options: z.array(z.string()).default([]),
  output_options: z.array(z.string()).default([]),
  faststart: z.boolean().default(true),
  overwrite: z.boolean().default(true),
  map_streams: z.array(z.string()).default([]),
  metadata: z.array(z.tuple([z.string(), z.string()])).default([]),
  remove_input_on_success: z.boolean().default(false).optional(),
});

// --- Rclone Processor ---
export const RcloneOperationSchema = z.enum(['copy', 'move', 'sync']);

export const RcloneConfigSchema = z.object({
  rclone_path: z.string().default('rclone'),
  max_retries: z.number().default(3),
  destination_root: z.string().optional(),
  config_path: z.string().optional(),
  remote_path: z.string().optional(), // Legacy support or direct override
  operation: RcloneOperationSchema.default('copy'),
  args: z.array(z.string()).default([]),
});

// --- Thumbnail Processor ---
export const ThumbnailConfigSchema = z.object({
  timestamp_secs: z.number().min(0).default(10),
  width: z.number().positive().default(320), // Match backend default
  quality: z.number().min(1).max(31).default(2), // 1-31 for qscale
  output_pattern: z.string().optional(),
});

// --- Audio Extract Processor ---
export const AudioFormatSchema = z.enum(['mp3', 'aac', 'flac', 'opus']);

export const AudioExtractConfigSchema = z.object({
  format: AudioFormatSchema.optional(), // If null -> copy
  bitrate: z.string().optional(),
  sample_rate: z.number().optional(),
  channels: z.number().optional(),
  output_path: z.string().optional(),
  overwrite: z.boolean().default(true),
});

// --- Compression Processor ---
export const ArchiveFormatSchema = z.enum(['zip', 'targz']);

export const CompressionConfigSchema = z.object({
  format: ArchiveFormatSchema.default('zip'),
  compression_level: z.number().min(0).max(9).default(6),
  output_path: z.string().optional(),
  overwrite: z.boolean().default(true),
  preserve_paths: z.boolean().default(false),
});

// --- Copy/Move Processor ---
export const CopyMoveOperationSchema = z.enum(['copy', 'move']);

export const CopyMoveConfigSchema = z.object({
  operation: CopyMoveOperationSchema.default('copy'),
  destination: z.string().optional(),
  create_dirs: z.boolean().default(true),
  verify_integrity: z.boolean().default(true),
  overwrite: z.boolean().default(false),
});

// --- Delete Processor ---
export const DeleteConfigSchema = z.object({
  max_retries: z.number().default(3),
  retry_delay_ms: z.number().default(100),
});

// --- Metadata Processor ---
export const MetadataConfigSchema = z.object({
  artist: z.string().optional(),
  title: z.string().optional(),
  date: z.string().optional(),
  album: z.string().optional(),
  comment: z.string().optional(),
  custom: z.record(z.string(), z.string()).default({}),
  output_path: z.string().optional(),
  overwrite: z.boolean().default(true),
});

// --- Execute Processor ---
export const ExecuteConfigSchema = z.object({
  command: z.string().min(1),
  // Directory to scan for new files after command execution
  scan_output_dir: z.string().optional(),
  // File extension filter for scanning (e.g., "mp4", "mkv")
  scan_extension: z.string().optional(),
});

// --- DanmakuFactory Processor ---
export const DanmakuFactoryConfigSchema = z.object({
  binary_path: z.string().optional(),
  args: z.array(z.string()).default(['-i', '{input}', '-o', '{output}']),
  extra_args: z.array(z.string()).default([]),
  overwrite: z.boolean().default(true),
  verify_output_exists: z.boolean().default(true),
  prefer_manifest: z.boolean().default(true),
  passthrough_inputs: z.boolean().default(true),
  delete_source_xml_on_success: z.boolean().default(false),
});

// --- ASS Burn-in Processor ---
export const AssMatchStrategySchema = z.enum(['manifest', 'stem']);

export const AssBurninConfigSchema = z.object({
  ffmpeg_path: z.string().optional(),
  match_strategy: AssMatchStrategySchema.default('manifest'),
  require_ass: z.boolean().default(true),
  passthrough_inputs: z.boolean().default(true),
  exclude_ass_from_passthrough: z.boolean().default(false),
  output_extension: z.string().optional(),
  video_codec: z.string().default('libx264'),
  audio_codec: z.string().default('copy'),
  crf: z.number().min(0).max(51).default(23),
  preset: PresetSchema.default('veryfast'),
  overwrite: z.boolean().default(true),
  fonts_dir: z.string().optional(),
  delete_source_videos_on_success: z.boolean().default(false),
  delete_source_ass_on_success: z.boolean().default(false),
});

// --- TDL Processor ---
export const TdlConfigSchema = TdlProcessorConfigSchema;
