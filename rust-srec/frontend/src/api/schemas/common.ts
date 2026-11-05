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

/**
 * `z.preprocess` step for a JSON-text column whose value must still match its
 * schema strictly: text that is not JSON becomes `null` instead of throwing,
 * so one unreadable row no longer fails a whole list response. The backend
 * cannot use such text either, so nothing is lost when a save overwrites it.
 * Valid JSON that fails the schema still fails the parse, unlike
 * `jsonTextField`, because degrading it would let the next whole-object save
 * delete a value the backend accepts.
 */
export function decodeJsonText(field: string) {
  return (value: unknown): unknown => {
    if (typeof value !== 'string') return value;
    if (value.trim() === '') return null;
    try {
      return JSON.parse(value);
    } catch (error) {
      console.warn(`Discarding unreadable JSON in "${field}":`, error);
      return null;
    }
  };
}

// --- Priority Enum ---
/** One page of a list endpoint: its items and the window and total they come from. */
export function paginated<Item extends z.ZodType>(item: Item) {
  return z.object({
    items: z.array(item),
    total: z.number(),
    limit: z.number(),
    offset: z.number(),
  });
}

export const PrioritySchema = z
  .enum(['HIGH', 'NORMAL', 'LOW'])
  .default('NORMAL');
export type Priority = z.infer<typeof PrioritySchema>;

// --- Shared Config Objects ---

// Mirrors `StreamSelectionConfig`: only `preferred_formats` is an `Option`, which the backend
// writes as `null` when unset. The other fields reject `null` there too.
export const StreamSelectionConfigObjectSchema = z.object({
  preferred_formats: z.array(z.string()).nullish(),
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

/**
 * Danmu statistics as an override layer's form submits them.
 *
 * The statistics inputs create the object as soon as they render, so a layer whose fields were
 * all left empty still holds `{}`. For an override that would replace the inherited settings with
 * the defaults, so it is sent as `null` and the layer keeps inheriting. The global layer has
 * nothing to inherit and keeps `{}`.
 */
export const DanmuStatisticsOverrideSchema =
  DanmuStatisticsObjectSchema.nullable()
    .optional()
    .overwrite((value) =>
      value && Object.values(value).every((field) => field === undefined)
        ? null
        : value,
    );

export const DownloadRetryPolicyObjectSchema = z.object({
  max_retries: z.number(),
  initial_delay_ms: z.number(),
  max_delay_ms: z.number(),
  backoff_multiplier: z.number(),
  use_jitter: z.boolean(),
});

/**
 * A stored retry policy as the backend reads it: `RetryPolicy` fills every absent field with its
 * serde default, so a partial object is a complete policy. The defaults are copied here so the
 * form shows the values the backend applies, and the output type stays the complete policy the
 * forms expect.
 */
export const DownloadRetryPolicyReadSchema = z.object({
  max_retries: z.number().default(3),
  initial_delay_ms: z.number().default(1000),
  max_delay_ms: z.number().default(30000),
  backoff_multiplier: z.number().default(2),
  use_jitter: z.boolean().default(true),
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
