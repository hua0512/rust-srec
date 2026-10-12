import { z } from 'zod';

// --- Engine-Specific Config Schemas ---

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

export const FfmpegConfigSchema = z.object({
  enable_lossless_cutting: z.boolean().default(false),
  binary_path: z.string().default('ffmpeg'),
  input_args: z.array(z.string()).default([]),
  output_args: z.array(z.string()).default([]),
  timeout_secs: z.coerce.number().int().min(0).default(30),
  graceful_stop_timeout_secs: z.coerce.number().int().min(0).default(60),
  user_agent: z
    .string()
    .nullable()
    .optional()
    .transform((val) => val ?? undefined),
});
export type FfmpegConfig = z.infer<typeof FfmpegConfigSchema>;

export const StreamlinkConfigSchema = z.object({
  enable_lossless_cutting: z.boolean().default(false),
  binary_path: z.string().default('streamlink'),
  ffmpeg_path: z.string().nullable().optional(),
  quality: z.string().default('best'),
  extra_args: z.array(z.string()).default([]),
  graceful_stop_timeout_secs: z.coerce.number().int().min(0).default(60),
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

export const MesioHttpVersionPreferenceSchema = z.enum([
  'auto',
  'http2_only',
  'http1_only',
]);

export const MesioDownloaderBaseOverrideSchema = z.object({
  timeout_ms: optionalInt(0),
  connect_timeout_ms: optionalInt(0),
  read_timeout_ms: optionalInt(0),
  write_timeout_ms: optionalInt(0),
  follow_redirects: z.boolean().optional(),
  user_agent: optionalNonEmptyString(),
  params: z.array(z.array(z.string()).length(2)).optional(),
  danger_accept_invalid_certs: z.boolean().optional(),
  force_ipv4: z.boolean().optional(),
  force_ipv6: z.boolean().optional(),
  http_version: MesioHttpVersionPreferenceSchema.optional(),
  http2_keep_alive_interval_ms: optionalInt(0),
  pool_max_idle_per_host: optionalInt(0),
  pool_idle_timeout_ms: optionalInt(0),
});

export const MesioHlsVariantSelectionPolicySchema = z.discriminatedUnion(
  'type',
  [
    z.object({ type: z.literal('highest_bitrate') }),
    z.object({ type: z.literal('lowest_bitrate') }),
    z.object({
      type: z.literal('closest_to_bitrate'),
      target_bitrate: z.coerce.number().int().min(0),
    }),
    z.object({ type: z.literal('audio_only') }),
    z.object({ type: z.literal('video_only') }),
    z.object({
      type: z.literal('matching_resolution'),
      width: z.coerce.number().int(),
      height: z.coerce.number().int(),
    }),
    z.object({ type: z.literal('custom'), value: z.string() }),
  ],
);

export const MesioHlsPlaylistConfigOverrideSchema = z.object({
  initial_playlist_fetch_timeout_ms: optionalInt(0),
  live_refresh_interval_ms: optionalInt(0),
  live_max_refresh_retries: optionalInt(0),
  live_refresh_retry_delay_ms: optionalInt(0),
  variant_selection_policy: MesioHlsVariantSelectionPolicySchema.optional(),
  adaptive_refresh_enabled: z.boolean().optional(),
  adaptive_refresh_min_interval_ms: optionalInt(0),
  adaptive_refresh_max_interval_ms: optionalInt(0),
});

export const MesioHlsSchedulerConfigOverrideSchema = z.object({
  download_concurrency: optionalInt(1),
  processed_segment_buffer_multiplier: optionalInt(0),
});

export const MesioHlsFetcherConfigOverrideSchema = z.object({
  segment_download_timeout_ms: optionalInt(0),
  max_segment_retries: optionalInt(0),
  segment_retry_delay_base_ms: optionalInt(0),
  max_segment_retry_delay_ms: optionalInt(0),
  key_download_timeout_ms: optionalInt(0),
  max_key_retries: optionalInt(0),
  key_retry_delay_base_ms: optionalInt(0),
  max_key_retry_delay_ms: optionalInt(0),
});

export const MesioHlsProcessorConfigOverrideSchema = z.object({
  processed_segment_ttl_ms: optionalInt(0),
});

export const MesioHlsDecryptionConfigOverrideSchema = z.object({
  key_cache_ttl_ms: optionalInt(0),
  offload_decryption_to_cpu_pool: z.boolean().optional(),
});

export const MesioHlsCacheConfigOverrideSchema = z.object({
  playlist_ttl_ms: optionalInt(0),
  segment_ttl_ms: optionalInt(0),
  decryption_key_ttl_ms: optionalInt(0),
});

export const MesioBufferLimitsOverrideSchema = z.object({
  max_segments: optionalInt(0),
  max_bytes: optionalInt(0),
});

export const MesioGapSkipStrategySchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('wait_indefinitely') }),
  z.object({
    type: z.literal('skip_after_count'),
    count: z.coerce.number().int().min(0),
  }),
  z.object({
    type: z.literal('skip_after_duration'),
    duration_ms: z.coerce.number().int().min(0),
  }),
  z.object({
    type: z.literal('skip_after_both'),
    count: z.coerce.number().int().min(0),
    duration_ms: z.coerce.number().int().min(0),
  }),
]);

export const NullableIntSchema = z
  .union([z.number(), z.string(), z.null()])
  .transform((val) => {
    // For tri-state fields:
    // - undefined/missing => leave unset (use default)
    // - null            => explicitly disable/clear
    // - number          => set value
    // Treat empty input as "unset" (undefined), not "disabled".
    if (val === '') return undefined;
    if (val === null) return null;
    return typeof val === 'number' ? val : Number(val);
  })
  .refine(
    (val) =>
      val === undefined ||
      val === null ||
      (Number.isFinite(val) && Number.isInteger(val)),
    { message: 'Must be an integer, null, or empty' },
  );

export const MesioHlsOutputConfigOverrideSchema = z.object({
  live_reorder_buffer_duration_ms: optionalInt(0),
  live_reorder_buffer_max_segments: optionalInt(0),
  gap_evaluation_interval_ms: optionalInt(0),
  max_pending_init_segments: optionalInt(0),
  live_max_overall_stall_duration_ms: NullableIntSchema.optional(),
  live_gap_strategy: MesioGapSkipStrategySchema.optional(),
  vod_gap_strategy: MesioGapSkipStrategySchema.optional(),
  vod_segment_timeout_ms: NullableIntSchema.optional(),
  buffer_limits: MesioBufferLimitsOverrideSchema.optional(),
  metrics_enabled: z.boolean().optional(),
});

export const MesioHlsConfigSchema = z.object({
  base: MesioDownloaderBaseOverrideSchema.optional(),
  playlist_config: MesioHlsPlaylistConfigOverrideSchema.optional(),
  scheduler_config: MesioHlsSchedulerConfigOverrideSchema.optional(),
  fetcher_config: MesioHlsFetcherConfigOverrideSchema.optional(),
  processor_config: MesioHlsProcessorConfigOverrideSchema.optional(),
  decryption_config: MesioHlsDecryptionConfigOverrideSchema.optional(),
  cache_config: MesioHlsCacheConfigOverrideSchema.optional(),
  output_config: MesioHlsOutputConfigOverrideSchema.optional(),
});

/**
 * A settings object as a template override sees it: every field, at every depth, may also be
 * `null`, which resets that setting (or that whole group) to the engine default. The backend
 * merge removes null-valued keys, so an override cannot express an engine's own `null`, such as
 * a disabled timeout.
 */
function allowResets(schema: z.ZodObject): z.ZodObject {
  const shape = Object.fromEntries(
    Object.entries(schema.shape).map(([key, field]) => {
      const inner = field instanceof z.ZodOptional ? field.unwrap() : field;
      return [
        key,
        inner instanceof z.ZodObject
          ? allowResets(inner).nullable().optional()
          : field.nullable(),
      ];
    }),
  );
  return z.object(shape);
}

const MesioHlsConfigOverrideSchema = allowResets(MesioHlsConfigSchema);

// --- Mesio MPEG-DASH ---
// Every field is optional and an unset field takes the engine default below. In a template
// override an absent field inherits the engine's value, while `null` resets it to the engine
// default (the backend merge removes null-valued keys); overrides therefore keep `null`, and the
// engine configuration treats it as unset.

const U32_MAX = 4_294_967_295;

/** The mesio engine's MPEG-DASH defaults, used when a field is unset. */
export const MESIO_DASH_DEFAULTS = {
  video_quality: { type: 'highest' },
  audio_quality: { type: 'highest' },
  include_video: true,
  include_audio: true,
  download_concurrency: 5,
  manifest_timeout_ms: 15000,
  min_refresh_interval_ms: 1000,
  max_refresh_retries: 5,
  segment_timeout_ms: 10000,
  max_segment_retries: 3,
  live_start: { type: 'auto' },
} as const;

const dashInt = (
  keepNull: boolean,
  min: number,
  max: number = Number.MAX_SAFE_INTEGER,
) =>
  z
    .union([z.number(), z.string(), z.null()])
    .transform((val) => {
      if (val === null) return keepNull ? null : undefined;
      if (val === '') return undefined;
      return typeof val === 'number' ? val : Number(val);
    })
    .refine(
      (val) =>
        val == null || (Number.isInteger(val) && val >= min && val <= max),
      {
        message:
          min > 0
            ? 'Must be a positive whole number'
            : 'Must be a whole number of 0 or more',
      },
    )
    .optional();

const dashRequiredInt = (max: number) =>
  z.coerce
    .number()
    .int('Must be a positive whole number')
    .min(1, 'Must be a positive whole number')
    .max(max, 'Must be a positive whole number');

const dashBoolean = (keepNull: boolean) =>
  z
    .boolean()
    .nullable()
    .optional()
    .transform((val) => (val === null && keepNull ? null : (val ?? undefined)));

/** Language tag shape the backend accepts: 1–8 letters or digits per subtag, joined by '-'. */
const LANGUAGE_TAG = /^[A-Za-z0-9]{1,8}(?:-[A-Za-z0-9]{1,8})*$/;

export const MesioDashVideoQualitySchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('highest') }),
  z.object({ type: z.literal('lowest') }),
  z.object({ type: z.literal('max_height'), height: dashRequiredInt(U32_MAX) }),
  z.object({
    type: z.literal('closest_bandwidth'),
    bandwidth: dashRequiredInt(Number.MAX_SAFE_INTEGER),
  }),
]);
export type MesioDashVideoQuality = z.infer<typeof MesioDashVideoQualitySchema>;

export const MesioDashAudioQualitySchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('highest') }),
  z.object({ type: z.literal('lowest') }),
  z.object({
    type: z.literal('closest_bandwidth'),
    bandwidth: dashRequiredInt(Number.MAX_SAFE_INTEGER),
  }),
]);
export type MesioDashAudioQuality = z.infer<typeof MesioDashAudioQualitySchema>;

export const MesioDashLiveStartSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('auto') }),
  z.object({ type: z.literal('live_edge') }),
  z.object({
    type: z.literal('delay'),
    delay_ms: z.coerce
      .number()
      .int('Must be a whole number of 0 or more')
      .min(0, 'Must be a whole number of 0 or more')
      .max(Number.MAX_SAFE_INTEGER, 'Must be a whole number of 0 or more'),
  }),
]);
export type MesioDashLiveStart = z.infer<typeof MesioDashLiveStartSchema>;

const dashPolicy = <T extends z.ZodType>(schema: T, keepNull: boolean) =>
  schema
    .nullable()
    .optional()
    .transform((val) => (val === null && keepNull ? null : (val ?? undefined)));

const dashLanguage = (keepNull: boolean) =>
  z
    .string()
    .nullable()
    .optional()
    .transform((val) => {
      if (val === null) return keepNull ? null : undefined;
      const trimmed = val?.trim();
      return trimmed ? trimmed : undefined;
    })
    .refine((val) => val == null || LANGUAGE_TAG.test(val), {
      message: "Use a language tag such as 'en' or 'pt-BR'",
    });

const mesioDashShape = (keepNull: boolean) => ({
  video_quality: dashPolicy(MesioDashVideoQualitySchema, keepNull),
  audio_quality: dashPolicy(MesioDashAudioQualitySchema, keepNull),
  audio_language: dashLanguage(keepNull),
  include_video: dashBoolean(keepNull),
  include_audio: dashBoolean(keepNull),
  download_concurrency: dashInt(keepNull, 1),
  manifest_timeout_ms: dashInt(keepNull, 1),
  min_refresh_interval_ms: dashInt(keepNull, 1),
  max_refresh_retries: dashInt(keepNull, 0, U32_MAX),
  segment_timeout_ms: dashInt(keepNull, 1),
  max_segment_retries: dashInt(keepNull, 0, U32_MAX),
  live_start: dashPolicy(MesioDashLiveStartSchema, keepNull),
});

/** The engine rejects a configuration that records neither track. */
function requireATrack(
  value: { include_video?: boolean | null; include_audio?: boolean | null },
  ctx: z.RefinementCtx,
) {
  if (value.include_video === false && value.include_audio === false) {
    ctx.addIssue({
      code: 'custom',
      message: 'Record at least one of video and audio',
      path: ['include_audio'],
    });
  }
}

export const MesioDashConfigSchema = z
  .object(mesioDashShape(false))
  .superRefine(requireATrack);
export type MesioDashConfig = z.infer<typeof MesioDashConfigSchema>;

export const MesioDashConfigOverrideSchema = z
  .object(mesioDashShape(true))
  .strict()
  .superRefine(requireATrack);
export type MesioDashConfigOverride = z.input<
  typeof MesioDashConfigOverrideSchema
>;

/**
 * A form that registered the DASH fields but set none of them saves no `dash` object. A `null`
 * field is a reset, not an unset field, so it keeps the object.
 */
function unlessEmpty<T extends object>(value: T): T | undefined {
  return Object.values(value).some((field) => field !== undefined)
    ? value
    : undefined;
}

/** DASH settings with every field resolved: what a template override inherits. */
export type MesioDashSettings = {
  [K in keyof Omit<MesioDashConfig, 'audio_language'>]-?: NonNullable<
    MesioDashConfig[K]
  >;
} & { audio_language?: string };

/** An engine's DASH settings with its unset fields at the engine defaults. */
export function resolveDashSettings(
  engineDash: Partial<MesioDashConfig> | null | undefined,
): MesioDashSettings {
  const settings: Record<string, unknown> = { ...MESIO_DASH_DEFAULTS };
  for (const [key, value] of Object.entries(engineDash ?? {})) {
    if (value != null) settings[key] = value;
  }
  return settings as MesioDashSettings;
}

/**
 * Whether a template's DASH override, merged over the engine's settings the way the backend
 * merges it (absent inherits, `null` resets to the default), records neither video nor audio.
 */
export function dashOverrideDisablesBothTracks(
  inherited: MesioDashSettings,
  override: unknown,
): boolean {
  if (override === null || typeof override !== 'object') return false;
  const fields = override as Record<string, unknown>;
  const effective = (key: 'include_video' | 'include_audio') => {
    const value = fields[key];
    if (value === undefined) return inherited[key];
    if (value === null) return MESIO_DASH_DEFAULTS[key];
    return value;
  };
  return (
    effective('include_video') === false && effective('include_audio') === false
  );
}

export const MesioConfigSchema = z.object({
  buffer_size: z.coerce.number().int().min(1).default(8388608), // 8MB
  fix_flv: z.boolean().default(true),
  fix_hls: z.boolean().default(true),
  fix_dash: z.boolean().default(true),
  flv_fix: z
    .object({
      sequence_header_change_mode: z
        .enum(['crc32', 'semantic_signature'])
        .default('crc32'),
      drop_duplicate_sequence_headers: z.boolean().default(false),
      duplicate_tag_filtering: z.boolean().default(false),
      duplicate_tag_filter_config: z
        .object({
          window_capacity_tags: z.coerce.number().int().min(1).default(8192),
          window_capacity_bytes: z.coerce
            .number()
            .int()
            .min(0)
            .default(16777216),
          replay_backjump_threshold_ms: z.coerce
            .number()
            .int()
            .min(0)
            .default(2000),
          enable_replay_offset_matching: z.boolean().default(false),
        })
        .optional()
        .default({
          window_capacity_tags: 8192,
          window_capacity_bytes: 16777216,
          replay_backjump_threshold_ms: 2000,
          enable_replay_offset_matching: false,
        }),
    })
    .optional(),
  hls: MesioHlsConfigSchema.optional(),
  dash: MesioDashConfigSchema.nullable()
    .optional()
    .transform((val) => (val ? unlessEmpty(val) : undefined)),
});
export type MesioConfig = z.infer<typeof MesioConfigSchema>;

// --- Engine Override Schemas ---
// Used by template engine overrides. These are *partial* (no defaults) and
// strict (unknown keys fail), so typos don't silently get ignored.

export const FfmpegConfigOverrideSchema = z
  .object({
    enable_lossless_cutting: z.boolean().optional(),
    binary_path: optionalString(),
    input_args: z.array(z.string()).optional(),
    output_args: z.array(z.string()).optional(),
    timeout_secs: optionalInt(0),
    graceful_stop_timeout_secs: optionalInt(0),
    user_agent: optionalNonEmptyString(),
  })
  .strict();
export type FfmpegConfigOverride = z.infer<typeof FfmpegConfigOverrideSchema>;

export const StreamlinkConfigOverrideSchema = z
  .object({
    enable_lossless_cutting: z.boolean().optional(),
    binary_path: optionalString(),
    ffmpeg_path: z.string().nullable().optional(),
    quality: optionalString(),
    extra_args: z.array(z.string()).optional(),
    graceful_stop_timeout_secs: optionalInt(0),
    twitch_proxy_playlist: optionalNonEmptyString(),
    twitch_proxy_playlist_exclude: optionalNonEmptyString(),
  })
  .strict();
export type StreamlinkConfigOverride = z.infer<
  typeof StreamlinkConfigOverrideSchema
>;

// In the Mesio override, `null` at any level resets that setting or group to the engine default.

const MesioDuplicateTagFilterOverrideSchema = z
  .object({
    window_capacity_tags: optionalInt(1).nullable(),
    window_capacity_bytes: optionalInt(0).nullable(),
    replay_backjump_threshold_ms: optionalInt(0).nullable(),
    enable_replay_offset_matching: z.boolean().nullable().optional(),
  })
  .strict();

const MesioFlvFixOverrideSchema = z
  .object({
    sequence_header_change_mode: z
      .enum(['crc32', 'semantic_signature'])
      .nullable()
      .optional(),
    drop_duplicate_sequence_headers: z.boolean().nullable().optional(),
    duplicate_tag_filtering: z.boolean().nullable().optional(),
    duplicate_tag_filter_config:
      MesioDuplicateTagFilterOverrideSchema.nullable().optional(),
  })
  .strict();

export const MesioConfigOverrideSchema = z
  .object({
    buffer_size: optionalInt(1).nullable(),
    fix_flv: z.boolean().nullable().optional(),
    fix_hls: z.boolean().nullable().optional(),
    fix_dash: z.boolean().nullable().optional(),
    flv_fix: MesioFlvFixOverrideSchema.nullable().optional(),
    hls: MesioHlsConfigOverrideSchema.nullable().optional(),
    dash: MesioDashConfigOverrideSchema.nullable()
      .optional()
      .transform((val) => (val ? unlessEmpty(val) : val)),
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
          message: `Unknown engine type: ${String(engine_type)}`,
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
