import { z } from 'zod';

/**
 * Read a field the backend stores as JSON text, reporting an unusable value as
 * `null` instead of failing the surrounding parse.
 *
 * A row written by an earlier version, or edited by hand in the database, can
 * hold text that is not JSON or no longer matches `schema`. These fields are
 * parsed as part of list responses, so rejecting one row would blank the whole
 * page; degrading that single field to "not configured" keeps the rest of the
 * response usable. A value that arrives already decoded is accepted as is,
 * which is what the write paths and the config forms hand back.
 *
 * `field` names the column in the warning logged whenever a value degrades:
 * saving the row afterwards overwrites whatever was there, so the discarded
 * value has to be visible somewhere.
 */
export function jsonTextField<Schema extends z.ZodType>(
  field: string,
  schema: Schema,
) {
  return z
    .unknown()
    .transform((value): z.output<Schema> | null => {
      if (value === null) return null;
      let decoded: unknown = value;
      if (typeof value === 'string') {
        if (value.trim() === '') return null;
        try {
          decoded = JSON.parse(value);
        } catch (error) {
          console.warn(`Discarding unreadable JSON in "${field}":`, error);
          return null;
        }
      }
      // The backend writes the JSON text `null` for a column that is not
      // configured, so a decoded null is the expected value rather than a row
      // worth warning about.
      if (decoded === null) return null;
      const result = schema.safeParse(decoded);
      if (!result.success) {
        console.warn(
          `Discarding "${field}", which does not match its schema:`,
          result.error.issues,
        );
        return null;
      }
      return result.data;
    })
    .optional();
}

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
