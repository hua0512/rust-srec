import { z } from 'zod';
import { CredentialSelectionSchema } from './credential-profiles';
import {
  StreamSelectionConfigObjectSchema,
  DanmuStatisticsObjectSchema,
  DanmuStatisticsOverrideSchema,
  DownloadRetryPolicyObjectSchema,
  DownloadRetryPolicyReadSchema,
  jsonTextField,
} from './common';
import { ProxyRouteSchema } from './proxies';
import { DagPipelineDefinitionSchema } from './pipeline';
import {
  AllPlatformConfigsSchema,
  ExtractorSelectionSchema,
  extractorSelectionRead,
} from './platform-configs';

// --- Platform Config ---
export const PlatformConfigSchema = z.object({
  id: z.string(),
  name: z.string(),
  fetch_delay_ms: z.number().nullable().optional(),
  download_delay_ms: z.number().nullable().optional(),
  record_danmu: z.boolean().nullable().optional(),
  credential_selection: jsonTextField(
    'credential_selection',
    CredentialSelectionSchema,
  ),
  platform_specific_config: jsonTextField(
    'platform_specific_config',
    AllPlatformConfigsSchema,
  ),
  output_folder: z.string().nullable().optional(),
  output_filename_template: z.string().nullable().optional(),
  download_engine: z.string().nullable().optional(),
  // Lenient on read only: `PlatformConfigFormSchema` and the write schema restore the strict enum.
  extractor: extractorSelectionRead({ trim: true }),
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
    DownloadRetryPolicyReadSchema,
  ),
  /** The platform's route; absent on a write keeps the stored one. */
  proxy_route: ProxyRouteSchema.optional(),
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
  extractor: ExtractorSelectionSchema.nullable().optional(),
  credential_selection: CredentialSelectionSchema.optional(),
  stream_selection_config:
    StreamSelectionConfigObjectSchema.nullable().optional(),
  danmu_statistics: DanmuStatisticsOverrideSchema,
  download_retry_policy: DownloadRetryPolicyObjectSchema.nullable().optional(),
  pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  session_complete_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  paired_segment_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
});
