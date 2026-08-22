import { z } from 'zod';

// --- Priority Enum ---
export const PrioritySchema = z
  .enum(['HIGH', 'NORMAL', 'LOW'])
  .default('NORMAL');
export type Priority = z.infer<typeof PrioritySchema>;

// --- Shared Config Objects ---

export const StreamSelectionConfigObjectSchema = z.object({
  preferred_formats: z.array(z.string()).optional(),
  preferred_media_formats: z.array(z.string()).optional(),
  preferred_qualities: z.array(z.string()).optional(),
  preferred_cdns: z.array(z.string()).optional(),
  blacklisted_cdns: z.array(z.string()).optional(),
  min_bitrate: z.number().optional(),
  max_bitrate: z.number().optional(),
});

// Every field is optional so a partial override round-trips: the backend fills
// unspecified fields from its defaults.
export const DanmuStatisticsObjectSchema = z.object({
  enabled: z.boolean().optional(),
  top_talkers: z.number().optional(),
  top_words: z.number().optional(),
  top_gifts: z.number().optional(),
  talker_capacity: z.number().optional(),
  word_capacity: z.number().optional(),
  gift_capacity: z.number().optional(),
  rate_bucket_secs: z.number().optional(),
  extra_stop_words: z.array(z.string()).optional(),
});

export const DownloadRetryPolicyObjectSchema = z.object({
  max_retries: z.number(),
  initial_delay_ms: z.number(),
  max_delay_ms: z.number(),
  backoff_multiplier: z.number(),
  use_jitter: z.boolean(),
});

export const ProxyConfigObjectSchema = z.object({
  enabled: z.boolean().default(false).optional(),
  url: z.string().optional(),
  username: z.string().optional(),
  password: z.string().optional(),
  use_system_proxy: z.boolean().default(false).optional(),
});

// --- Pipeline Step Schemas ---
// Preset step: references a job preset by name
export const PresetPipelineStepSchema = z.object({
  type: z.literal('preset'),
  name: z.string(),
});

// Workflow step: references a pipeline workflow by name (expands to multiple steps)
export const WorkflowPipelineStepSchema = z.object({
  type: z.literal('workflow'),
  name: z.string(),
});

// Inline step: directly defines a processor with optional config
export const InlinePipelineStepSchema = z.object({
  type: z.literal('inline'),
  processor: z.string(),
  config: z.any().default({}).optional(),
});

// Union of all step types using discriminated union
export const PipelineStepSchema = z.discriminatedUnion('type', [
  PresetPipelineStepSchema,
  WorkflowPipelineStepSchema,
  InlinePipelineStepSchema,
]);
export type PipelineStep = z.infer<typeof PipelineStepSchema>;
