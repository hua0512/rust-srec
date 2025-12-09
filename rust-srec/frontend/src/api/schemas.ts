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
    platform_config_id: z.string().nullable().optional(),
    consecutive_error_count: z.number(),
    last_check_time: z.string().nullable().optional(),
    last_stream_time: z.string().nullable().optional(),
    created_at: z.string(),
    updated_at: z.string(),
});

export const CreateStreamerSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    url: z.url({ message: 'Invalid URL' }),
    platform_config_id: z.string().optional(),
    template_id: z.string().optional(),
    priority: z.enum(['HIGH', 'NORMAL', 'LOW']).default('NORMAL'),
    enabled: z.boolean().default(true),
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
    title: z.string(),
    start_time: z.string(),
    end_time: z.string().nullable().optional(),
    duration_seconds: z.number().nullable().optional(),
    status: z.string(), // 'Active' | 'Completed' | 'Error'
    output_count: z.number(),
    total_size_bytes: z.number(),
});

// --- Pipeline Schemas ---
export const JobSchema = z.object({
    id: z.string(),
    streamer_id: z.string(),
    session_id: z.string().nullable().optional(),
    status: z.enum(['Pending', 'Processing', 'Completed', 'Failed', 'Cancelled']),
    step: z.string(),
    progress: z.number().min(0).max(100).optional(),
    error_message: z.string().nullable().optional(),
    created_at: z.string(),
    started_at: z.string().nullable().optional(),
    completed_at: z.string().nullable().optional(),
});

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
});

export const ProxyConfigObjectSchema = z.object({
    enabled: z.boolean().default(false),
    url: z.string().optional(),
    username: z.string().optional(),
    password: z.string().optional(),
    use_system_proxy: z.boolean().default(false),
});

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
    max_bitrate: z.number().nullable().optional(),
    stream_selection_config: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    min_segment_size_bytes: z.number().nullable().optional(),
    max_download_duration_secs: z.number().nullable().optional(),
    max_part_size_bytes: z.number().nullable().optional(),
    download_retry_policy: z.string().nullable().optional(),
    event_hooks: z.string().nullable().optional(),
});

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
    max_bitrate: z.number().nullable().optional(),
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
});

export const CreateTemplateRequestSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    output_folder: z.string().nullable().optional(),
    output_filename_template: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    max_bitrate: z.number().nullable().optional(),
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
    path: z.string(),
    size_bytes: z.number(),
    format: z.string(),
    created_at: z.string(),
});

export const ExtractMetadataResponseSchema = z.object({
    platform: z.string().nullable(),
    valid_platform_configs: z.array(PlatformConfigSchema),
    channel_id: z.string().nullable(),
});
