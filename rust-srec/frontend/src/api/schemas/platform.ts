import { z } from 'zod';
import {
  StreamSelectionConfigObjectSchema,
  DanmuStatisticsObjectSchema,
  DownloadRetryPolicyObjectSchema,
  ProxyConfigObjectSchema,
  jsonTextField,
} from './common';
import { DagPipelineDefinitionSchema } from './pipeline';
import {
  AllPlatformConfigsSchema,
  ExtractorSelectionSchema,
} from './platform-configs';

// --- Platform Config ---
export const PlatformConfigSchema = z.object({
  id: z.string(),
  name: z.string(),
  fetch_delay_ms: z.number().nullable().optional(),
  download_delay_ms: z.number().nullable().optional(),
  record_danmu: z.boolean().nullable().optional(),
  cookies: z.string().nullable().optional(),
  platform_specific_config: jsonTextField(
    'platform_specific_config',
    AllPlatformConfigsSchema,
  ),
  output_folder: z.string().nullable().optional(),
  output_filename_template: z.string().nullable().optional(),
  download_engine: z.string().nullable().optional(),
  extractor: ExtractorSelectionSchema.nullable().optional(),
  output_file_format: z.string().nullable().optional(),
  min_segment_size_bytes: z.number().nullable().optional(),
  max_download_duration_secs: z.number().nullable().optional(),
  max_part_size_bytes: z.number().nullable().optional(),

  // Complex fields: Backend sends JSON string, we parse to object
  stream_selection_config: jsonTextField(
    'stream_selection_config',
    StreamSelectionConfigObjectSchema,
  ),
  danmu_statistics: jsonTextField(
    'danmu_statistics',
    DanmuStatisticsObjectSchema,
  ),
  download_retry_policy: jsonTextField(
    'download_retry_policy',
    DownloadRetryPolicyObjectSchema,
  ),
  proxy_config: jsonTextField('proxy_config', ProxyConfigObjectSchema),
  pipeline: jsonTextField('pipeline', DagPipelineDefinitionSchema),
  session_complete_pipeline: jsonTextField(
    'session_complete_pipeline',
    DagPipelineDefinitionSchema,
  ),
  paired_segment_pipeline: jsonTextField(
    'paired_segment_pipeline',
    DagPipelineDefinitionSchema,
  ),

  // Per-platform overrides for the global offline-confirmation cadence.
  // NULL = inherit from global. Floors mirror server-side
  // `HysteresisConfig::from_scheduler` clamping.
  offline_check_count: z.number().int().min(1).nullable().optional(),
  offline_check_delay_ms: z.number().int().min(1000).nullable().optional(),
});

export type PlatformConfig = z.infer<typeof PlatformConfigSchema>;

// Schema for Forms (expects objects, not JSON strings)
export const PlatformConfigFormSchema = PlatformConfigSchema.extend({
  stream_selection_config:
    StreamSelectionConfigObjectSchema.nullable().optional(),
  danmu_statistics: DanmuStatisticsObjectSchema.nullable().optional(),
  download_retry_policy: DownloadRetryPolicyObjectSchema.nullable().optional(),
  proxy_config: ProxyConfigObjectSchema.nullable().optional(),
  pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  session_complete_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  paired_segment_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
});
