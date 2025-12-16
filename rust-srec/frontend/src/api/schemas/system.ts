import { z } from 'zod';

// --- System Schemas ---
export const GlobalConfigSchema = z.object({
  output_folder: z.string(),
  output_filename_template: z.string(),
  output_file_format: z.string(),
  min_segment_size_bytes: z.number(),
  max_download_duration_secs: z.number(),
  max_part_size_bytes: z.number(),
  record_danmu: z.boolean(),
  max_concurrent_downloads: z.number(),
  max_concurrent_uploads: z.number(),
  streamer_check_delay_ms: z.number(),

  proxy_config: z.any().optional(),

  offline_check_delay_ms: z.number(),
  offline_check_count: z.number(),
  default_download_engine: z.string(),
  max_concurrent_cpu_jobs: z.number(),
  max_concurrent_io_jobs: z.number(),
  job_history_retention_days: z.number(),
  session_gap_time_secs: z.number(),

  pipeline: z.any().optional(),
});

export const ComponentHealthSchema = z.object({
  name: z.string(),
  status: z.string(),
  message: z.string().nullable().optional(),
  last_check: z.string().nullable().optional(),
  check_duration_ms: z.number().nullable().optional(),
});

export const HealthSchema = z.object({
  status: z.string(),
  version: z.string(),
  uptime_secs: z.number(),
  cpu_usage: z.number(),
  memory_usage: z.number(),
  components: z.array(ComponentHealthSchema).default([]),
});

export const PipelineStatsSchema = z.object({
  pending_count: z.number(),
  processing_count: z.number(),
  completed_count: z.number(),
  failed_count: z.number(),
  avg_processing_time_secs: z.number().nullable().optional(),
});

export const MediaOutputSchema = z.object({
  id: z.string(),
  session_id: z.string(),
  streamer_id: z.string(),
  file_path: z.string(),
  file_size_bytes: z.number(),
  duration_secs: z.number().nullable().optional(),
  format: z.string(),
  created_at: z.string(),
});
export type MediaOutput = z.infer<typeof MediaOutputSchema>;

// NOTE: This ExtractMetadataResponse is for URL extraction, NOT media file metadata
export const ExtractMetadataResponseSchema = z.object({
  platform: z.string().nullable().optional(),
  valid_platform_configs: z.array(z.any()), // PlatformConfigSchema array
  channel_id: z.string().nullable().optional(),
});
export type ExtractMetadataResponse = z.infer<
  typeof ExtractMetadataResponseSchema
>;

// --- Parse URL Schemas ---
export const ParseUrlRequestSchema = z.object({
  url: z.string(),
  cookies: z.string().optional(),
});
export type ParseUrlRequest = z.infer<typeof ParseUrlRequestSchema>;

export const ParseUrlResponseSchema = z.object({
  success: z.boolean(),
  is_live: z.boolean(),
  media_info: z.any().optional(),
  error: z.string().nullable().optional(),
});
export type ParseUrlResponse = z.infer<typeof ParseUrlResponseSchema>;

// --- Resolve URL Schemas ---
export const ResolveUrlRequestSchema = z.object({
  url: z.string(),
  stream_info: z.any(),
  cookies: z.string().optional(),
});
export type ResolveUrlRequest = z.infer<typeof ResolveUrlRequestSchema>;

export const ResolveUrlResponseSchema = z.object({
  success: z.boolean(),
  stream_info: z.any().optional(),
  error: z.string().nullable().optional(),
});
export type ResolveUrlResponse = z.infer<typeof ResolveUrlResponseSchema>;
