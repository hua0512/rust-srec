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

export const PipelineJobsPageResponseSchema = z.object({
    items: z.array(JobSchema),
    limit: z.number(),
    offset: z.number(),
});
export type PipelineJobsPageResponse = z.infer<
    typeof PipelineJobsPageResponseSchema
>;

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
    config: z.string(),
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

// --- Pipeline Presets (Workflows) ---

import { PipelineStepSchema } from './common';

export const PipelinePresetSchema = z.object({
    id: z.string(),
    name: z.string(),
    description: z.string().nullable().optional(),
    steps: z.array(PipelineStepSchema),
    created_at: z.string(),
    updated_at: z.string(),
});
export type PipelinePreset = z.infer<typeof PipelinePresetSchema>;

export const CreatePipelinePresetRequestSchema = z.object({
    name: z.string().min(1),
    description: z.string().nullable().optional(),
    steps: z.array(PipelineStepSchema),
});
export type CreatePipelinePresetRequest = z.infer<typeof CreatePipelinePresetRequestSchema>;

export const UpdatePipelinePresetRequestSchema = CreatePipelinePresetRequestSchema;
export type UpdatePipelinePresetRequest = z.infer<typeof UpdatePipelinePresetRequestSchema>;

export const PipelinePresetListResponseSchema = z.object({
    presets: z.array(PipelinePresetSchema),
    total: z.number(),
    limit: z.number(),
    offset: z.number(),
});
export type PipelinePresetListResponse = z.infer<typeof PipelinePresetListResponseSchema>;
