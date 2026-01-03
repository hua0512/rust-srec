import { z } from 'zod';

// --- Engine-Specific Config Schemas ---

export const FfmpegConfigSchema = z.object({
  binary_path: z.string().default('ffmpeg'),
  input_args: z.array(z.string()).default([]),
  output_args: z.array(z.string()).default([]),
  timeout_secs: z.coerce.number().int().min(0).default(30),
  user_agent: z
    .string()
    .nullable()
    .optional()
    .transform((val) => val ?? undefined),
});
export type FfmpegConfig = z.infer<typeof FfmpegConfigSchema>;

export const StreamlinkConfigSchema = z.object({
  binary_path: z.string().default('streamlink'),
  quality: z.string().default('best'),
  extra_args: z.array(z.string()).default([]),
  // Twitch proxy playlist (ttv-lol)
  twitch_proxy_playlist: z
    .string()
    .nullable()
    .optional()
    .transform((val) => (val?.trim() ? val.trim() : undefined)),
  twitch_proxy_playlist_exclude: z
    .string()
    .nullable()
    .optional()
    .transform((val) => (val?.trim() ? val.trim() : undefined)),
});
export type StreamlinkConfig = z.infer<typeof StreamlinkConfigSchema>;

export const MesioConfigSchema = z.object({
  buffer_size: z.coerce.number().int().min(1).default(8388608), // 8MB
  fix_flv: z.boolean().default(true),
  fix_hls: z.boolean().default(true),
  flv_fix: z
    .object({
      sequence_header_change_mode: z
        .enum(['crc32', 'semantic_signature'])
        .default('crc32'),
      drop_duplicate_sequence_headers: z.boolean().default(false),
      duplicate_tag_filtering: z.boolean().default(true),
      duplicate_tag_filter_config: z
        .object({
          window_capacity_tags: z.coerce.number().int().min(1).default(8192),
          replay_backjump_threshold_ms: z.coerce
            .number()
            .int()
            .min(0)
            .default(2000),
          enable_replay_offset_matching: z.boolean().default(true),
        })
        .optional()
        .default({
          window_capacity_tags: 8192,
          replay_backjump_threshold_ms: 2000,
          enable_replay_offset_matching: true,
        }),
    })
    .optional(),
});
export type MesioConfig = z.infer<typeof MesioConfigSchema>;

// --- Engine Override Schemas ---
// Used by template engine overrides. These are *partial* (no defaults) and
// strict (unknown keys fail), so typos don't silently get ignored.

const optionalNonEmptyString = () =>
  z
    .string()
    .transform((val) => {
      const trimmed = val.trim();
      return trimmed.length === 0 ? undefined : trimmed;
    })
    .optional();

const optionalString = () =>
  z
    .string()
    .transform((val) => (val.length === 0 ? undefined : val))
    .optional();

const optionalInt = (min: number) =>
  z
    .union([z.number(), z.string()])
    .transform((val) => {
      if (val === '') return undefined;
      return typeof val === 'number' ? val : Number(val);
    })
    .refine(
      (val) =>
        val === undefined ||
        (Number.isFinite(val) && Number.isInteger(val) && val >= min),
      { message: `Must be an integer >= ${min}` },
    )
    .optional();

export const FfmpegConfigOverrideSchema = z
  .object({
    binary_path: optionalString(),
    input_args: z.array(z.string()).optional(),
    output_args: z.array(z.string()).optional(),
    timeout_secs: optionalInt(0),
    user_agent: optionalNonEmptyString(),
  })
  .strict();
export type FfmpegConfigOverride = z.infer<typeof FfmpegConfigOverrideSchema>;

export const StreamlinkConfigOverrideSchema = z
  .object({
    binary_path: optionalString(),
    quality: optionalString(),
    extra_args: z.array(z.string()).optional(),
    twitch_proxy_playlist: optionalNonEmptyString(),
    twitch_proxy_playlist_exclude: optionalNonEmptyString(),
  })
  .strict();
export type StreamlinkConfigOverride = z.infer<
  typeof StreamlinkConfigOverrideSchema
>;

const MesioDuplicateTagFilterOverrideSchema = z
  .object({
    window_capacity_tags: optionalInt(1),
    replay_backjump_threshold_ms: optionalInt(0),
    enable_replay_offset_matching: z.boolean().optional(),
  })
  .strict();

const MesioFlvFixOverrideSchema = z
  .object({
    sequence_header_change_mode: z
      .enum(['crc32', 'semantic_signature'])
      .optional(),
    drop_duplicate_sequence_headers: z.boolean().optional(),
    duplicate_tag_filtering: z.boolean().optional(),
    duplicate_tag_filter_config:
      MesioDuplicateTagFilterOverrideSchema.optional(),
  })
  .strict();

export const MesioConfigOverrideSchema = z
  .object({
    buffer_size: optionalInt(1),
    fix_flv: z.boolean().optional(),
    fix_hls: z.boolean().optional(),
    flv_fix: MesioFlvFixOverrideSchema.optional(),
  })
  .strict();
export type MesioConfigOverride = z.infer<typeof MesioConfigOverrideSchema>;

export const EngineConfigOverrideSchema = z.union([
  FfmpegConfigOverrideSchema,
  StreamlinkConfigOverrideSchema,
  MesioConfigOverrideSchema,
]);
export type EngineConfigOverride = z.infer<typeof EngineConfigOverrideSchema>;

// --- Engine Configuration with Discriminated Union ---
// Provides complete type safety based on engine_type

export const EngineConfigSchema = z.discriminatedUnion('engine_type', [
  z.object({
    id: z.string(),
    name: z.string(),
    engine_type: z.literal('FFMPEG'),
    config: FfmpegConfigSchema,
  }),
  z.object({
    id: z.string(),
    name: z.string(),
    engine_type: z.literal('STREAMLINK'),
    config: StreamlinkConfigSchema,
  }),
  z.object({
    id: z.string(),
    name: z.string(),
    engine_type: z.literal('MESIO'),
    config: MesioConfigSchema,
  }),
]);
export type EngineConfig = z.infer<typeof EngineConfigSchema>;

// --- Engine Type Enum ---
export const EngineTypeSchema = z.enum(['FFMPEG', 'STREAMLINK', 'MESIO']);
export type EngineType = z.infer<typeof EngineTypeSchema>;

// --- Create Request Schema ---
// Uses superRefine for config validation based on engine_type
// This provides react-hook-form compatibility while maintaining runtime validation
export const CreateEngineRequestSchema = z
  .object({
    name: z.string().min(1, 'Name is required'),
    engine_type: EngineTypeSchema,
    config: z.record(z.string(), z.unknown()),
  })
  .superRefine((data, ctx) => {
    const { engine_type, config } = data;

    let result;
    switch (engine_type) {
      case 'FFMPEG':
        result = FfmpegConfigSchema.safeParse(config);
        break;
      case 'STREAMLINK':
        result = StreamlinkConfigSchema.safeParse(config);
        break;
      case 'MESIO':
        result = MesioConfigSchema.safeParse(config);
        break;
      default:
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          message: `Unknown engine type: ${engine_type}`,
          path: ['engine_type'],
        });
        return;
    }

    if (!result.success) {
      result.error.issues.forEach((issue) => {
        ctx.addIssue({
          ...issue,
          path: ['config', ...issue.path],
        });
      });
    }
  });
export type CreateEngineRequest = z.infer<typeof CreateEngineRequestSchema>;

// --- Update Request Schema ---
export const UpdateEngineRequestSchema = z
  .object({
    name: z.string().min(1, 'Name is required').optional(),
    engine_type: EngineTypeSchema.optional(),
    config: z.record(z.string(), z.unknown()).optional(),
    version: z.string().optional(),
  })
  .superRefine((data, ctx) => {
    if (!data.engine_type || !data.config) return;

    let result;
    switch (data.engine_type) {
      case 'FFMPEG':
        result = FfmpegConfigSchema.safeParse(data.config);
        break;
      case 'STREAMLINK':
        result = StreamlinkConfigSchema.safeParse(data.config);
        break;
      case 'MESIO':
        result = MesioConfigSchema.safeParse(data.config);
        break;
    }

    if (result && !result.success) {
      result.error.issues.forEach((issue) => {
        ctx.addIssue({
          ...issue,
          path: ['config', ...issue.path],
        });
      });
    }
  });
export type UpdateEngineRequest = z.infer<typeof UpdateEngineRequestSchema>;

// --- Test Response Schema ---
export const EngineTestResponseSchema = z.object({
  available: z.boolean(),
  version: z.string().optional(),
});
export type EngineTestResponse = z.infer<typeof EngineTestResponseSchema>;
