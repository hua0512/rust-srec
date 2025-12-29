import { z } from 'zod';
import {
  StreamSelectionConfigObjectSchema,
  DownloadRetryPolicyObjectSchema,
  ProxyConfigObjectSchema,
  EventHooksSchema,
} from './common';
import { DagPipelineDefinitionSchema } from './pipeline';
import { AllPlatformConfigsSchema } from './platform-configs';

// --- Platform Config ---
export const PlatformConfigSchema = z.object({
  id: z.string(),
  name: z.string(),
  fetch_delay_ms: z.number().nullable().optional(),
  download_delay_ms: z.number().nullable().optional(),
  record_danmu: z.boolean().nullable().optional(),
  cookies: z.string().nullable().optional(),
  platform_specific_config: z
    .preprocess((val) => {
      if (typeof val === 'string' && val.trim() !== '') {
        try {
          return JSON.parse(val);
        } catch (e) {
          console.error(e);
          return val;
        }
      }
      return val;
    }, AllPlatformConfigsSchema.nullable().optional())
    .nullable()
    .optional(),
  output_folder: z.string().nullable().optional(),
  output_filename_template: z.string().nullable().optional(),
  download_engine: z.string().nullable().optional(),
  output_file_format: z.string().nullable().optional(),
  min_segment_size_bytes: z.number().nullable().optional(),
  max_download_duration_secs: z.number().nullable().optional(),
  max_part_size_bytes: z.number().nullable().optional(),

  // Complex fields: Backend sends JSON string, we parse to object
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
    .transform((str) => JSON.parse(str))
    .pipe(DagPipelineDefinitionSchema.nullable().optional())
    .nullable()
    .optional(),
  session_complete_pipeline: z
    .string()
    .transform((str) => JSON.parse(str))
    .pipe(DagPipelineDefinitionSchema.nullable().optional())
    .nullable()
    .optional(),
  paired_segment_pipeline: z
    .string()
    .transform((str) => JSON.parse(str))
    .pipe(DagPipelineDefinitionSchema.nullable().optional())
    .nullable()
    .optional(),
});

export type PlatformConfig = z.infer<typeof PlatformConfigSchema>;

// Schema for Forms (expects objects, not JSON strings)
export const PlatformConfigFormSchema = PlatformConfigSchema.extend({
  stream_selection_config:
    StreamSelectionConfigObjectSchema.nullable().optional(),
  download_retry_policy: DownloadRetryPolicyObjectSchema.nullable().optional(),
  proxy_config: ProxyConfigObjectSchema.nullable().optional(),
  event_hooks: EventHooksSchema.nullable().optional(),
  pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  session_complete_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
  paired_segment_pipeline: DagPipelineDefinitionSchema.nullable().optional(),
});
