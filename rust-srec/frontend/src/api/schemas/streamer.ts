import { z } from 'zod';
import {
  StreamSelectionConfigObjectSchema,
  DownloadRetryPolicyObjectSchema,
  DanmuSamplingConfigObjectSchema,
  EventHooksSchema,
  PrioritySchema,
} from './common';
import { DagPipelineDefinitionSchema } from './pipeline';

// --- Streamer Schemas ---
export const StreamerStateSchema = z.enum([
  'NOT_LIVE',
  'LIVE',
  'OUT_OF_SCHEDULE',
  'OUT_OF_SPACE',
  'FATAL_ERROR',
  'CANCELLED',
  'NOT_FOUND',
  'INSPECTING_LIVE',
  'TEMPORAL_DISABLED',
  'ERROR',
  'DISABLED',
]);

// Streamer config overrides
export const StreamerSpecificConfigSchema = z.object({
  // Use z.preprocess for all complex fields that might come as JSON strings
  stream_selection_config: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      StreamSelectionConfigObjectSchema.nullable().optional(),
    )
    .nullable()
    .optional(),

  proxy_config: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      z.any().nullable().optional(),
    )
    .nullable()
    .optional(),

  download_retry_policy: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      DownloadRetryPolicyObjectSchema.nullable().optional(),
    )
    .nullable()
    .optional(),

  danmu_sampling_config: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      DanmuSamplingConfigObjectSchema.nullable().optional(),
    )
    .nullable()
    .optional(),

  event_hooks: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      EventHooksSchema.nullable().optional(),
    )
    .nullable()
    .optional(),

  pipeline: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      DagPipelineDefinitionSchema.nullable().optional(),
    )
    .nullable()
    .optional(),
  session_complete_pipeline: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      DagPipelineDefinitionSchema.nullable().optional(),
    )
    .nullable()
    .optional(),
  paired_segment_pipeline: z
    .preprocess(
      (val) => (typeof val === 'string' ? JSON.parse(val) : val),
      DagPipelineDefinitionSchema.nullable().optional(),
    )
    .nullable()
    .optional(),

  output_folder: z
    .preprocess((v) => (v === '' ? null : v), z.string().nullable().optional())
    .nullable()
    .optional(),
  output_filename_template: z
    .preprocess((v) => (v === '' ? null : v), z.string().nullable().optional())
    .nullable()
    .optional(),
  output_file_format: z
    .preprocess((v) => (v === '' ? null : v), z.string().nullable().optional())
    .nullable()
    .optional(),
  min_segment_size_bytes: z
    .preprocess(
      (v) => (v === '' ? null : typeof v === 'string' ? Number(v) : v),
      z.number().nullable().optional(),
    )
    .nullable()
    .optional(),
  max_download_duration_secs: z
    .preprocess(
      (v) => (v === '' ? null : typeof v === 'string' ? Number(v) : v),
      z.number().nullable().optional(),
    )
    .nullable()
    .optional(),
  max_part_size_bytes: z
    .preprocess(
      (v) => (v === '' ? null : typeof v === 'string' ? Number(v) : v),
      z.number().nullable().optional(),
    )
    .nullable()
    .optional(),
  record_danmu: z.boolean().nullable().optional(),
  cookies: z
    .preprocess((v) => (v === '' ? null : v), z.string().nullable().optional())
    .nullable()
    .optional(),
  download_engine: z
    .preprocess((v) => (v === '' ? null : v), z.string().nullable().optional())
    .nullable()
    .optional(),
  engines_override: z
    .preprocess((v) => (v === '' ? null : v), z.string().nullable().optional())
    .nullable()
    .optional(),
});

export const StreamerSchema = z.object({
  id: z.string(),
  name: z.string(),
  url: z.string(),
  avatar_url: z.string().nullable().optional(),
  platform_config_id: z.string(),
  template_id: z.string().nullable().optional(),
  state: StreamerStateSchema,
  priority: PrioritySchema,
  enabled: z.boolean(),
  consecutive_error_count: z.number(),
  disabled_until: z.string().nullable().optional(),
  last_error: z.string().nullable().optional(),
  last_live_time: z.string().nullable().optional(),
  created_at: z.string(),
  updated_at: z.string(),
  streamer_specific_config: StreamerSpecificConfigSchema.nullable().optional(),
});

export const CreateStreamerSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  url: z.url('Invalid URL'),
  platform_config_id: z.string(),
  template_id: z.string().nullable().optional(),
  priority: PrioritySchema.optional(),
  enabled: z.boolean().default(true),
  streamer_specific_config: StreamerSpecificConfigSchema.nullable().optional(),
});

export const UpdateStreamerSchema = CreateStreamerSchema.partial();
export const StreamerFormSchema = CreateStreamerSchema;
export type StreamerFormValues = z.infer<typeof StreamerFormSchema>;
