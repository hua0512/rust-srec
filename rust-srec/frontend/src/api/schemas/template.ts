import { z } from 'zod';
import { ExtractorSelectionSchema } from './platform-configs';
import {
  StreamSelectionConfigObjectSchema,
  DanmuStatisticsObjectSchema,
  DownloadRetryPolicyObjectSchema,
  ProxyConfigObjectSchema,
  jsonTextField,
} from './common';
import { DagPipelineDefinitionSchema } from './pipeline';
import { EngineConfigOverrideSchema } from './engine';

// Read schema: be permissive (don't break loading older/hand-edited templates).
const EnginesOverrideReadSchema = jsonTextField(
  z.record(z.string(), z.record(z.string(), z.any())),
);

// Write schema: validate + coerce known override fields (engine forms write into this).
const EnginesOverrideWriteSchema = z
  .record(z.string(), EngineConfigOverrideSchema)
  .optional();

// --- Template ---
export const TemplateSchema = z.object({
  id: z.string(),
  name: z.string(),
  output_folder: z.string().nullable().optional(),
  output_filename_template: z.string().nullable().optional(),
  output_file_format: z.string().nullable().optional(),
  download_engine: z.string().nullable().optional(),
  extractor: ExtractorSelectionSchema.nullable().optional(),
  record_danmu: z.boolean().nullable().optional(),
  platform_overrides: jsonTextField(z.any()),
  engines_override: EnginesOverrideReadSchema,
  min_segment_size_bytes: z.number().nullable().optional(),
  max_download_duration_secs: z.number().nullable().optional(),
  max_part_size_bytes: z.number().nullable().optional(),
  cookies: z.string().nullable().optional(),
  stream_selection_config: jsonTextField(StreamSelectionConfigObjectSchema),
  danmu_statistics: jsonTextField(DanmuStatisticsObjectSchema),
  download_retry_policy: jsonTextField(DownloadRetryPolicyObjectSchema),
  proxy_config: jsonTextField(ProxyConfigObjectSchema),
  pipeline: jsonTextField(DagPipelineDefinitionSchema),
  session_complete_pipeline: jsonTextField(DagPipelineDefinitionSchema),
  paired_segment_pipeline: jsonTextField(DagPipelineDefinitionSchema),
  // Per-template overrides for the offline-confirmation cadence.
  offline_check_count: z.number().int().min(1).nullable().optional(),
  offline_check_delay_ms: z.number().int().min(1000).nullable().optional(),
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
  extractor: ExtractorSelectionSchema.nullable().optional(),
  platform_overrides: z.any().nullable().optional(),
  engines_override: EnginesOverrideWriteSchema.optional(),
  stream_selection_config:
    StreamSelectionConfigObjectSchema.nullable().optional(),
  danmu_statistics: DanmuStatisticsObjectSchema.nullable().optional(),
  download_retry_policy: DownloadRetryPolicyObjectSchema.nullable().optional(),
  proxy_config: ProxyConfigObjectSchema.nullable().optional(),
  pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  session_complete_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  paired_segment_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  offline_check_count: z.number().int().min(1).nullable().optional(),
  offline_check_delay_ms: z.number().int().min(1000).nullable().optional(),
});
export const UpdateTemplateRequestSchema = CreateTemplateRequestSchema;
export const TemplateFormSchema = CreateTemplateRequestSchema;
