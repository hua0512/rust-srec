import { z } from 'zod';
import {
    StreamSelectionConfigObjectSchema,
    DownloadRetryPolicyObjectSchema,
    DanmuSamplingConfigObjectSchema,
    ProxyConfigObjectSchema,
    EventHooksSchema,
    PipelineStepSchema,
} from './common';

// --- Template ---
export const TemplateSchema = z.object({
    id: z.string(),
    name: z.string(),
    output_folder: z.string().nullable().optional(),
    output_filename_template: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    download_engine: z.string().nullable().optional(),
    record_danmu: z.boolean().nullable().optional(),
    platform_overrides: z.any().nullable().optional(),
    engines_override: z.any().nullable().optional(),
    min_segment_size_bytes: z.number().nullable().optional(),
    max_download_duration_secs: z.number().nullable().optional(),
    max_part_size_bytes: z.number().nullable().optional(),
    cookies: z.string().nullable().optional(),
    stream_selection_config: z
        .string()
        .transform((str) => JSON.parse(str))
        .pipe(StreamSelectionConfigObjectSchema.nullable().optional())
        .nullable()
        .optional(),
    download_retry_policy: z
        .string()
        .transform((str) => JSON.parse(str))
        .pipe(DownloadRetryPolicyObjectSchema.nullable().optional())
        .nullable()
        .optional(),
    danmu_sampling_config: z
        .string()
        .transform((str) => JSON.parse(str))
        .pipe(DanmuSamplingConfigObjectSchema.nullable().optional())
        .nullable()
        .optional(),
    proxy_config: z
        .string()
        .transform((str) => JSON.parse(str))
        .pipe(ProxyConfigObjectSchema.nullable().optional())
        .nullable()
        .optional(),
    event_hooks: z
        .string()
        .transform((str) => JSON.parse(str))
        .pipe(EventHooksSchema.nullable().optional())
        .nullable()
        .optional(),
    pipeline: z
        .string()
        .transform((str) => {
            try {
                return JSON.parse(str);
            } catch {
                return null;
            }
        })
        .pipe(z.array(PipelineStepSchema).nullable().optional())
        .nullable()
        .optional(),
    usage_count: z.number().optional(),
    created_at: z.string().optional(),
    updated_at: z.string().optional(),
});

export type Template = z.infer<typeof TemplateSchema>;

export const CreateTemplateRequestSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    // All usage fields are optional overrides
    output_folder: z.string().nullable().optional(),
    output_filename_template: z.string().nullable().optional(),
    output_file_format: z.string().nullable().optional(),
    min_segment_size_bytes: z.number().nullable().optional(),
    max_download_duration_secs: z.number().nullable().optional(),
    max_part_size_bytes: z.number().nullable().optional(),
    record_danmu: z.boolean().nullable().optional(),
    cookies: z.string().nullable().optional(),
    download_engine: z.string().nullable().optional(),
    platform_overrides: z.any().nullable().optional(),
    engines_override: z.any().nullable().optional(),
    stream_selection_config: StreamSelectionConfigObjectSchema.nullable().optional(),
    download_retry_policy: DownloadRetryPolicyObjectSchema.nullable().optional(),
    danmu_sampling_config: DanmuSamplingConfigObjectSchema.nullable().optional(),
    proxy_config: ProxyConfigObjectSchema.nullable().optional(),
    event_hooks: EventHooksSchema.nullable().optional(),
    pipeline: z.array(PipelineStepSchema).nullable().optional(),
});
export const UpdateTemplateRequestSchema = CreateTemplateRequestSchema;
export const TemplateFormSchema = CreateTemplateRequestSchema;
