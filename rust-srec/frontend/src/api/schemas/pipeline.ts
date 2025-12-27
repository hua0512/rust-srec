import { z } from 'zod';

// --- Pipeline Schemas ---
export const JobLogEntrySchema = z.object({
  timestamp: z.string(),
  level: z.string(),
  message: z.string(),
});

export const StepDurationInfoSchema = z.object({
  step: z.number(),
  processor: z.string(),
  duration_secs: z.number(),
  started_at: z.string(),
  completed_at: z.string(),
});
export type StepDurationInfo = z.infer<typeof StepDurationInfoSchema>;

export const JobExecutionInfoSchema = z.object({
  current_processor: z.string().nullable().optional(),
  current_step: z.number().nullable().optional(),
  total_steps: z.number().nullable().optional(),
  items_produced: z.array(z.string()),
  input_size_bytes: z.number().nullable().optional(),
  output_size_bytes: z.number().nullable().optional(),
  logs: z.array(JobLogEntrySchema),
  step_durations: z.array(StepDurationInfoSchema).default([]),
  log_lines_total: z.number().default(0),
  log_warn_count: z.number().default(0),
  log_error_count: z.number().default(0),
});
export type JobExecutionInfo = z.infer<typeof JobExecutionInfoSchema>;

export const JobStatusSchema = z.enum([
  'PENDING',
  'PROCESSING',
  'COMPLETED',
  'FAILED',
  'INTERRUPTED',
]);
export type JobStatus = z.infer<typeof JobStatusSchema>;

export const JobSchema = z.object({
  id: z.string(),
  session_id: z.string(),
  streamer_id: z.string(),
  streamer_name: z.string().nullable().optional(),
  pipeline_id: z.string().nullable().optional(),
  status: JobStatusSchema,
  processor_type: z.string(),
  input_path: z.array(z.string()),
  output_path: z.array(z.string()).nullish(),
  error_message: z.string().nullable().optional(),
  progress: z.number().nullable().optional(),
  created_at: z.string(),
  started_at: z.string().nullable().optional(),
  completed_at: z.string().nullable().optional(),
  execution_info: JobExecutionInfoSchema.nullable().optional(),
  duration_secs: z.number().nullable().optional(),
  queue_wait_secs: z.number().nullable().optional(),
});
export type Job = z.infer<typeof JobSchema>;

export const JobLogsResponseSchema = z.object({
  items: z.array(JobLogEntrySchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});
export type JobLogsResponse = z.infer<typeof JobLogsResponseSchema>;

export const JobPresetSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string().nullable().optional(),
  category: z.string().nullable().optional(),
  processor: z.string(),
  config: z.string().transform((str) => {
    try {
      return JSON.parse(str);
    } catch {
      return {};
    }
  }),
  created_at: z.string(),
  updated_at: z.string(),
});
export type JobPreset = z.infer<typeof JobPresetSchema>;

// Valid processor types for presets
export const VALID_PROCESSORS = [
  'remux',
  'rclone',
  'thumbnail',
  'execute',
  'audio_extract',
  'compression',
  'copy_move',
  'delete',
  'metadata',
  'danmaku_factory',
  'ass_burnin',
] as const;
export type ProcessorType = (typeof VALID_PROCESSORS)[number];

export const VALID_CATEGORIES = [
  'remux', // Container format conversion (no re-encoding)
  'compression', // Re-encoding/transcoding
  'thumbnail', // Image/preview generation
  'audio', // Audio extraction
  'archive', // Archiving/compression
  'upload', // Cloud upload (rclone)
  'cleanup', // File deletion
  'file_ops', // Copy/move operations
  'custom', // Custom execute commands
  'metadata', // Metadata operations
] as const;
export type PresetCategory = (typeof VALID_CATEGORIES)[number];

// --- DAG Pipeline Schemas ---

export const DagStatusSchema = z.enum([
  'PENDING',
  'PROCESSING',
  'COMPLETED',
  'FAILED',
  'CANCELLED',
]);
export type DagStatus = z.infer<typeof DagStatusSchema>;

export const DagStepStatusSchema = z.enum([
  'BLOCKED',
  'PENDING',
  'PROCESSING',
  'COMPLETED',
  'FAILED',
  'CANCELLED',
]);
export type DagStepStatus = z.infer<typeof DagStepStatusSchema>;

export const DagStepSchema = z.object({
  step_id: z.string(),
  status: DagStepStatusSchema,
  job_id: z.string().nullable().optional(),
  depends_on: z.array(z.string()),
  outputs: z.array(z.string()),
  processor: z.string(),
});
export type DagStep = z.infer<typeof DagStepSchema>;

export const DagExecutionSchema = z.object({
  id: z.string(),
  name: z.string(),
  status: DagStatusSchema,
  streamer_id: z.string().nullable().optional(),
  session_id: z.string().nullable().optional(),
  total_steps: z.number(),
  completed_steps: z.number(),
  failed_steps: z.number(),
  progress_percent: z.number(),
  steps: z.array(DagStepSchema),
  error: z.string().nullable().optional(),
  created_at: z.string(),
  updated_at: z.string(),
  completed_at: z.string().nullable().optional(),
});
export type DagExecution = z.infer<typeof DagExecutionSchema>;

export const DagGraphNodeSchema = z.object({
  id: z.string(),
  label: z.string(),
  status: DagStepStatusSchema,
  processor: z.string().nullable().optional(),
  job_id: z.string().nullable().optional(),
});
export type DagGraphNode = z.infer<typeof DagGraphNodeSchema>;

export const DagGraphEdgeSchema = z.object({
  from: z.string(),
  to: z.string(),
});
export type DagGraphEdge = z.infer<typeof DagGraphEdgeSchema>;

export const DagGraphSchema = z.object({
  dag_id: z.string(),
  name: z.string(),
  nodes: z.array(DagGraphNodeSchema),
  edges: z.array(DagGraphEdgeSchema),
});
export type DagGraph = z.infer<typeof DagGraphSchema>;

export const DagStatsSchema = z.object({
  dag_id: z.string(),
  blocked: z.number(),
  pending: z.number(),
  processing: z.number(),
  completed: z.number(),
  failed: z.number(),
  cancelled: z.number(),
  total: z.number(),
  progress_percent: z.number(),
});
export type DagStats = z.infer<typeof DagStatsSchema>;

export const DagSummarySchema = z.object({
  id: z.string(),
  name: z.string(),
  status: DagStatusSchema,
  streamer_id: z.string().nullable().optional(),
  streamer_name: z.string().nullable().optional(),
  session_id: z.string().nullable().optional(),
  total_steps: z.number(),
  completed_steps: z.number(),
  failed_steps: z.number(),
  progress_percent: z.number(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type DagSummary = z.infer<typeof DagSummarySchema>;

export const DagListResponseSchema = z.object({
  dags: z.array(DagSummarySchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});
export type DagListResponse = z.infer<typeof DagListResponseSchema>;

export const DagStepDefinitionSchema = z.object({
  id: z.string(),
  step: PipelineStepSchema,
  depends_on: z.array(z.string()).optional(),
});
export type DagStepDefinition = z.infer<typeof DagStepDefinitionSchema>;

export const DagPipelineDefinitionSchema = z.object({
  name: z.string(),
  steps: z.array(DagStepDefinitionSchema),
});
export type DagPipelineDefinition = z.infer<typeof DagPipelineDefinitionSchema>;

// --- Pipeline Presets (Workflows) ---

import { PipelineStepSchema } from './common';

export const PipelinePresetSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string().nullable().optional(),
  dag: DagPipelineDefinitionSchema,
  created_at: z.string(),
  updated_at: z.string(),
});
export type PipelinePreset = z.infer<typeof PipelinePresetSchema>;

export const CreatePipelinePresetRequestSchema = z.object({
  name: z.string().min(1),
  description: z.string().nullable().optional(),
  dag: DagPipelineDefinitionSchema,
});
export type CreatePipelinePresetRequest = z.infer<
  typeof CreatePipelinePresetRequestSchema
>;

export const PipelinePresetPreviewSchema = z.object({
  preset_id: z.string(),
  preset_name: z.string(),
  jobs: z.array(
    z.object({
      step_id: z.string(),
      processor: z.string(),
      depends_on: z.array(z.string()),
      is_root: z.boolean(),
      is_leaf: z.boolean(),
    }),
  ),
  execution_order: z.array(z.string()),
});
export type PipelinePresetPreview = z.infer<typeof PipelinePresetPreviewSchema>;

export const UpdatePipelinePresetRequestSchema =
  CreatePipelinePresetRequestSchema;
export type UpdatePipelinePresetRequest = z.infer<
  typeof UpdatePipelinePresetRequestSchema
>;

export const PipelinePresetListResponseSchema = z.object({
  presets: z.array(PipelinePresetSchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});
export type PipelinePresetListResponse = z.infer<
  typeof PipelinePresetListResponseSchema
>;
