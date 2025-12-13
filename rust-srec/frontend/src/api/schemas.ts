import { z } from 'zod';

// --- Auth Schemas ---
export const LoginRequestSchema = z.object({
    username: z.string().min(1, 'Username is required'),
    password: z.string().min(1, 'Password is required'),
    device_info: z.string().optional(),
});

export const RefreshRequestSchema = z.object({
    refresh_token: z.string(),
});


export const LoginResponseSchema = z.object({
    access_token: z.string(),
    refresh_token: z.string(),
    token_type: z.string(),
    expires_in: z.number(),
    refresh_expires_in: z.number(),
    roles: z.array(z.string()),
    must_change_password: z.boolean(),
});

export const ChangePasswordRequestSchema = z.object({
    current_password: z.string().min(1, 'Current password is required'),
    new_password: z.string().min(8, 'Password must be at least 8 characters'),
    confirm_password: z.string().min(1, 'Confirm password is required'),
}).refine((data) => data.new_password === data.confirm_password, {
    message: "Passwords don't match",
    path: ["confirm_password"],
});

// --- Streamer Schemas ---
export const StreamerSchema = z.object({
    id: z.string(),
    name: z.string(),
    url: z.url(),
    avatar_url: z.string().nullable().optional(),
    state: z.string(),
    enabled: z.boolean(),
    priority: z.enum(['HIGH', 'NORMAL', 'LOW']),
    template_id: z.string().nullable().optional(),
    platform_config_id: z.string(),
    consecutive_error_count: z.number(),
    disabled_until: z.string().nullable().optional(),
    last_error: z.string().nullable().optional(),
    last_live_time: z.string().nullable().optional(),
    created_at: z.string(),

    updated_at: z.string(),
    streamer_specific_config: z.string().nullable().optional(),
    download_retry_policy: z.string().nullable().optional(),
    danmu_sampling_config: z.string().nullable().optional(),
});

export const CreateStreamerSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    url: z.url({ message: 'Invalid URL' }),
    platform_config_id: z.string().optional(),
    template_id: z.string().optional(),
    priority: z.enum(['HIGH', 'NORMAL', 'LOW']).default('NORMAL'),
    enabled: z.boolean().default(true),
    streamer_specific_config: z.string().optional(),
    download_retry_policy: z.string().optional(),
    danmu_sampling_config: z.string().optional(),
});

export const UpdateStreamerSchema = CreateStreamerSchema.partial();

// --- Filter Schemas ---
export const FilterTypeSchema = z.enum([
    'TIME_BASED',
    'KEYWORD',
    'CATEGORY',
    'CRON',
    'REGEX',
]);

// TimeBased filter config
export const TimeBasedFilterConfigSchema = z.object({
    days: z.array(z.string()), // e.g. ["Mon", "Tue"]
    start_time: z.string(), // "HH:MM:SS"
    end_time: z.string(), // "HH:MM:SS"
});

// Keyword filter config
export const KeywordFilterConfigSchema = z.object({
    keywords: z.array(z.string()),
    exclude: z.boolean().default(false),
    case_sensitive: z.boolean().default(false),
});

// Category filter config
export const CategoryFilterConfigSchema = z.object({
    categories: z.array(z.string()),
    exclude: z.boolean().default(false),
});

// Cron filter config
export const CronFilterConfigSchema = z.object({
    expression: z.string(),
    timezone: z.string().optional(),
});

// Regex filter config
export const RegexFilterConfigSchema = z.object({
    pattern: z.string(),
    exclude: z.boolean().default(false),
    case_insensitive: z.boolean().default(false),
});

// Union of all filter types for 'config' field
// Since the backend uses a generic JSON Value for config, we can use z.any() or discriminated union if we had a discriminator field inside config.
// However, the discriminator is `filter_type` which is outside `config`.
// So for now, we'll use z.any() for the general FilterSchema or try to be specific if we can.
export const FilterSchema = z.object({
    id: z.string(),
    streamer_id: z.string(),
    filter_type: FilterTypeSchema,
    config: z.any(), // We will cast this to specific type in UI based on filter_type
});

export const CreateFilterRequestSchema = z.object({
    filter_type: FilterTypeSchema,
    config: z.any(),
});

export const UpdateFilterRequestSchema = z.object({
    filter_type: FilterTypeSchema.optional(),
    config: z.any().optional(),
});

// --- Session Schemas ---
export const SessionSchema = z.object({
    id: z.string(),
    streamer_id: z.string(),
    streamer_name: z.string(),
    streamer_avatar: z.string().nullable().optional(),
    titles: z.array(z.object({
        title: z.string(),
        timestamp: z.string(),
    })),
    title: z.string(),
    start_time: z.string(),
    end_time: z.string().nullable().optional(),
    duration_secs: z.number().nullable().optional(),
    output_count: z.number(),
    total_size_bytes: z.number(),
    danmu_count: z.number().nullable().optional(),
    thumbnail_url: z.string().nullable().optional(),
});

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
});
export type JobExecutionInfo = z.infer<typeof JobExecutionInfoSchema>;

export const JobStatusSchema = z.enum(['PENDING', 'PROCESSING', 'COMPLETED', 'FAILED', 'CANCELLED', 'INTERRUPTED']);
export type JobStatus = z.infer<typeof JobStatusSchema>;

export const JobSchema = z.object({
    id: z.string(),
    streamer_id: z.string(),
    session_id: z.string().nullable().optional(),
    pipeline_id: z.string().nullable().optional(),
    status: JobStatusSchema,
    processor_type: z.string(),
    input_path: z.array(z.string()),
    output_path: z.array(z.string()).nullish(),
    progress: z.number().min(0).max(100).optional(),
    error_message: z.string().nullable().optional(),
    created_at: z.string(),
    started_at: z.string().nullable().optional(),
    completed_at: z.string().nullable().optional(),
    execution_info: JobExecutionInfoSchema.nullable().optional(),
    duration_secs: z.number().nullable().optional(),
    queue_wait_secs: z.number().nullable().optional(),
});
export type Job = z.infer<typeof JobSchema>;


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
    "remux",
    "rclone",
    "thumbnail",
    "execute",
    "audio_extract",
    "compression",
    "copy_move",
    "delete",
    "metadata",
] as const;
export type ProcessorType = typeof VALID_PROCESSORS[number];

// Valid preset categories
export const VALID_CATEGORIES = [
    "remux",       // Container format conversion (no re-encoding)
    "compression", // Re-encoding/transcoding
    "thumbnail",   // Image/preview generation
    "audio",       // Audio extraction
    "archive",     // Archiving/compression
    "upload",      // Cloud upload (rclone)
    "cleanup",     // File deletion
    "file_ops",    // Copy/move operations
    "custom",      // Custom execute commands
    "metadata",    // Metadata operations
] as const;
export type PresetCategory = typeof VALID_CATEGORIES[number];

// --- Config Schemas ---
export const StreamSelectionConfigObjectSchema = z.object({
    preferred_formats: z.array(z.string()).optional(),
    preferred_media_formats: z.array(z.string()).optional(),
    preferred_qualities: z.array(z.string()).optional(),
    preferred_cdns: z.array(z.string()).optional(),
    min_bitrate: z.number().optional(),
    max_bitrate: z.number().optional(),
});

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
    max_concurrent_cpu_jobs: z.number(),
    max_concurrent_io_jobs: z.number(),
    streamer_check_delay_ms: z.number(),
    proxy_config: z.string().nullable().optional(),
    offline_check_delay_ms: z.number(),
    offline_check_count: z.number(),
    default_download_engine: z.string(),
    job_history_retention_days: z.number(),
    session_gap_time_secs: z.number(),
    pipeline: z.string().nullable().optional(),
});

export const ProxyConfigObjectSchema = z.object({
    enabled: z.boolean().default(false),
    url: z.string().optional(),
    username: z.string().optional(),
    password: z.string().optional(),
    use_system_proxy: z.boolean().default(false),
});

// Event hooks for streamer lifecycle events
// Each hook is a single command string to execute
export const EventHooksSchema = z.object({
    on_online: z.string().optional(),
    on_offline: z.string().optional(),
    on_download_start: z.string().optional(),
    on_download_complete: z.string().optional(),
    on_download_error: z.string().optional(),
    on_pipeline_complete: z.string().optional(),
});
export type EventHooks = z.infer<typeof EventHooksSchema>;

export const PlatformConfigSchema = z.object({
    id: z.string(),
    name: z.string(),
    fetch_delay_ms: z.number().nullable().optional(),
    download_delay_ms: z.number().nullable().optional(),
    record_danmu: z.boolean().nullable().optional(),
    cookies: z.string().nullable().optional(),
    platform_specific_config: z.string().nullable().optional(),
    proxy_config: z.string().nullable().optional(),
    output_folder: z.string().nullable().optional(),
    output_filename_template: z.string().nullable().optional(),
    download_engine: z.string().nullable().optional(),
    stream_selection_config: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    min_segment_size_bytes: z.number().nullable().optional(),
    max_download_duration_secs: z.number().nullable().optional(),
    max_part_size_bytes: z.number().nullable().optional(),
    download_retry_policy: z.string().nullable().optional(),
    event_hooks: z.string().nullable().optional(),
    pipeline: z.string().nullable().optional(),
});
export type PlatformConfig = z.infer<typeof PlatformConfigSchema>;

export const EngineConfigSchema = z.object({
    id: z.string(),
    name: z.string(),
    engine_type: z.enum(['FFMPEG', 'STREAMLINK', 'MESIO']),
    config: z.string(),
});


export const CreateEngineRequestSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    engine_type: z.enum(['FFMPEG', 'STREAMLINK', 'MESIO']),
    config: z.any(), // The backend expects a JSON value, and the client deserializes the string to pass object
});

export const UpdateEngineRequestSchema = CreateEngineRequestSchema.partial();

export const TemplateSchema = z.object({
    id: z.string(),
    name: z.string(),
    output_folder: z.string().nullable().optional(),
    output_filename_template: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    min_segment_size_bytes: z.number().nullable().optional(),
    max_download_duration_secs: z.number().nullable().optional(),
    max_part_size_bytes: z.number().nullable().optional(),
    record_danmu: z.boolean().nullable().optional(),
    cookies: z.string().nullable().optional(),
    platform_overrides: z.any().optional(),
    download_retry_policy: z.string().nullable().optional(),
    danmu_sampling_config: z.string().nullable().optional(),
    download_engine: z.string().nullable().optional(),
    engines_override: z.any().optional(),
    proxy_config: z.string().nullable().optional(),
    event_hooks: z.string().nullable().optional(),
    stream_selection_config: z.string().nullable().optional(),
    usage_count: z.number(),
    created_at: z.string(),
    updated_at: z.string(),
    streamer_specific_config: z.string().nullable().optional(),
    pipeline: z.string().nullable().optional(),
});
export type Template = z.infer<typeof TemplateSchema>;

export const CreateTemplateRequestSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    output_folder: z.string().nullable().optional(),
    output_filename_template: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    min_segment_size_bytes: z.number().nullable().optional(),
    max_download_duration_secs: z.number().nullable().optional(),
    max_part_size_bytes: z.number().nullable().optional(),
    record_danmu: z.boolean().nullable().optional(),
    cookies: z.string().nullable().optional(),
    platform_overrides: z.any().optional(),
    download_retry_policy: z.string().nullable().optional(),
    danmu_sampling_config: z.string().nullable().optional(),
    download_engine: z.string().nullable().optional(),
    engines_override: z.any().optional(),
    proxy_config: z.string().nullable().optional(),
    event_hooks: z.string().nullable().optional(),
    stream_selection_config: z.string().nullable().optional(),
    pipeline: z.string().nullable().optional(),
});

export const UpdateTemplateRequestSchema = CreateTemplateRequestSchema.partial();

// --- System & Stats Schemas ---
// --- System & Stats Schemas ---
export const HealthSchema = z.object({
    status: z.string(),
    version: z.string(),
    uptime_secs: z.number(),
    cpu_usage: z.number(),
    memory_usage: z.number(),
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
    file_path: z.string(),
    file_size_bytes: z.number(),
    format: z.string(),
    created_at: z.string(),
});

export const ExtractMetadataResponseSchema = z.object({
    platform: z.string().nullable(),
    valid_platform_configs: z.array(PlatformConfigSchema),
    channel_id: z.string().nullable(),
});

// --- Pipeline Step Schemas ---
// Inline pipeline step with processor and config
export const InlinePipelineStepSchema = z.object({
    processor: z.string(),
    config: z.any().default({}),
});

// Pipeline step can be either a preset name (string) or inline definition
// Uses untagged serialization in Rust, so:
// - Preset: just a string like "remux"
// - Inline: object with processor and config
export const PipelineStepSchema = z.union([
    z.string(), // Preset name
    InlinePipelineStepSchema, // Inline definition
]);
export type PipelineStep = z.infer<typeof PipelineStepSchema>;

// --- Streamer Specific Config Schema ---
// This is the JSON object stored in streamer_specific_config field
export const StreamerSpecificConfigSchema = z.object({
    // Output settings
    output_folder: z.string().optional(),
    output_filename_template: z.string().optional(),
    output_file_format: z.string().optional(),

    // Size and duration limits
    min_segment_size_bytes: z.number().optional(),
    max_download_duration_secs: z.number().optional(),
    max_part_size_bytes: z.number().optional(),

    // Recording settings
    record_danmu: z.boolean().optional(),
    download_engine: z.string().optional(),
    cookies: z.string().optional(),

    // Proxy configuration
    proxy_config: ProxyConfigObjectSchema.optional(),

    // Stream selection
    stream_selection: StreamSelectionConfigObjectSchema.optional(),

    // Event hooks - each hook is a single command string
    event_hooks: EventHooksSchema.optional(),

    // Pipeline configuration - array of steps
    pipeline: z.array(PipelineStepSchema).optional(),
});
export type StreamerSpecificConfig = z.infer<typeof StreamerSpecificConfigSchema>;
