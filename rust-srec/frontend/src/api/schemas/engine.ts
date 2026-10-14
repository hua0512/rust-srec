import { z } from 'zod';

/*
 * Engine settings are saved as the user set them. A field left unset is left out, so the backend
 * applies its own default, and a later change of that default reaches every engine that never
 * chose a value. A field therefore reads an emptied input, and `null`, as unset; the form shows
 * the defaults from its own field lists without writing them.
 *
 * Settings groups keep keys the form does not know: the backend accepts more than the form edits,
 * and dropping one when an engine or template is saved would change its behaviour.
 */

const U32_MAX = 4_294_967_295;

/** A whole number from `min` to `max`, typed into a number input or sent by the backend. */
const intSetting = (min: number, max: number = Number.MAX_SAFE_INTEGER) =>
  z
    .union([z.number(), z.string(), z.null()])
    .transform((val) => {
      if (val === null || (typeof val === 'string' && val.trim() === '')) {
        return undefined;
      }
      return typeof val === 'number' ? val : Number(val);
    })
    .refine(
      (val) =>
        val === undefined ||
        (Number.isInteger(val) && val >= min && val <= max),
      {
        message:
          min > 0
            ? 'Must be a positive whole number'
            : 'Must be a whole number of 0 or more',
      },
    )
    .optional();

/**
 * Text, trimmed, and unset when that leaves it empty. A `literal` value, such as a path, is kept
 * exactly as stored, even empty: the backend reads an empty path as a path, not as unset. The
 * form leaves a field the user clears unset rather than empty.
 */
const textSetting = ({ literal = false }: { literal?: boolean } = {}) =>
  z
    .string()
    .nullable()
    .optional()
    .transform((val) => {
      if (val == null) return undefined;
      if (literal) return val;
      const text = val.trim();
      return text === '' ? undefined : text;
    });

const boolSetting = () =>
  z
    .boolean()
    .nullable()
    .optional()
    .transform((val) => val ?? undefined);

const listSetting = () =>
  z
    .array(z.string())
    .nullable()
    .optional()
    .transform((val) => val ?? undefined);

/** A value of `schema`, such as a choice or a group of settings; `null` leaves it unset. */
const optionalSetting = <T extends z.ZodType>(schema: T) =>
  schema
    .nullable()
    .optional()
    .transform((val) => val ?? undefined);

/**
 * A timeout an engine can disable: `null` disables it, unlike other fields. A template override
 * cannot, as the backend merge reads `null` there as "use the default".
 */
const disableableIntSetting = () =>
  z
    .union([z.number(), z.string(), z.null()])
    .transform((val) => {
      if (val === null) return null;
      if (typeof val === 'string' && val.trim() === '') return undefined;
      return typeof val === 'number' ? val : Number(val);
    })
    .refine((val) => val == null || (Number.isInteger(val) && val >= 0), {
      message: 'Must be a whole number of 0 or more',
    })
    .optional();

const group = <T extends z.ZodRawShape>(shape: T) =>
  z.object(shape).catchall(z.json());

/**
 * `value` without unset fields or groups left empty by them. A form that registered a group's
 * fields but set none of them would otherwise save the empty group. `null` is a reset and stays.
 */
function withoutEmptyGroups<T>(value: T): T {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return value;
  }
  const entries = Object.entries(value).flatMap(([key, field]) => {
    const pruned = withoutEmptyGroups(field as unknown);
    if (pruned === undefined) return [];
    if (
      pruned !== null &&
      typeof pruned === 'object' &&
      !Array.isArray(pruned) &&
      Object.keys(pruned).length === 0
    ) {
      return [];
    }
    return [[key, pruned]];
  });
  return Object.fromEntries(entries) as T;
}

/** An engine's settings object, saved without unset fields. */
const engineSettings = <T extends z.ZodRawShape>(shape: T) =>
  group(shape).transform(withoutEmptyGroups);

/**
 * `schema`, refusing an emptied input. Numeric fields read `''` as unset, so a cleared input would
 * otherwise save a value the form does not show.
 */
function rejectBlank<T extends z.ZodType>(
  schema: T,
  message = 'Enter a value',
): T {
  // Typed as `schema`: the check accepts nothing `schema` would not, so its input and output
  // types are `schema`'s.
  return z
    .unknown()
    .refine((value) => typeof value !== 'string' || value.trim() !== '', {
      message,
    })
    .pipe(schema) as unknown as T;
}

/** A required whole number inside a policy, such as a target bitrate. */
const policyInt = (min: number, max: number = Number.MAX_SAFE_INTEGER) =>
  rejectBlank(
    z.coerce
      .number()
      .int(
        min > 0
          ? 'Must be a positive whole number'
          : 'Must be a whole number of 0 or more',
      )
      .min(
        min,
        min > 0
          ? 'Must be a positive whole number'
          : 'Must be a whole number of 0 or more',
      )
      .max(
        max,
        min > 0
          ? 'Must be a positive whole number'
          : 'Must be a whole number of 0 or more',
      ),
  );

// --- Template overrides ---

/**
 * An override field set to a custom value must hold one: an emptied input would otherwise save
 * as an absent field, which inherits the engine's value while the form still shows "Custom".
 * Every override field may be absent.
 */
function requireValue<T extends z.ZodType>(field: T): T {
  return rejectBlank(
    field,
    'Enter a value, or choose another option',
  ).optional() as unknown as T;
}

/** The settings group a field holds, behind the wrappers that let it be unset. */
function groupOf(field: z.ZodType): z.ZodObject | undefined {
  let inner: unknown = field;
  for (;;) {
    if (inner instanceof z.ZodObject) return inner;
    if (inner instanceof z.ZodOptional || inner instanceof z.ZodNullable) {
      inner = inner.unwrap();
    } else if (inner instanceof z.ZodPipe) {
      inner = inner.in;
    } else {
      return undefined;
    }
  }
}

/** `shape` as a template override sees it: every field, and every group, may also be `null`. */
function resettableShape(shape: z.ZodRawShape): z.ZodRawShape {
  return Object.fromEntries(
    Object.entries(shape).map(([key, entry]) => {
      const field = entry as z.ZodType;
      const fields = groupOf(field);
      return [
        key,
        fields
          ? resettableGroup(fields).nullable().optional()
          : requireValue(field.nullable()),
      ];
    }),
  );
}

function resettableGroup(fields: z.ZodObject): z.ZodObject {
  const rebuilt = group(resettableShape(fields.shape));
  // Rules across a group's fields, such as recording at least one DASH track, still apply.
  const checks = (fields._zod.def.checks ?? []) as z.core.$ZodCheck<
    Record<string, unknown>
  >[];
  return checks.length > 0 ? rebuilt.check(...checks) : rebuilt;
}

/**
 * A template's override of an engine's settings: every field, at every depth, may also be `null`,
 * which resets that setting (or that whole group) to the engine default. The backend merge removes
 * null-valued keys, so an override cannot express an engine's own `null`, such as a disabled
 * timeout.
 *
 * Unknown keys are refused at the top level only: a template's overrides are checked against each
 * engine type in turn, and that refusal is what tells the types apart.
 */
function overrideSettings(shape: z.ZodRawShape) {
  return z
    .object(resettableShape(shape))
    .strict()
    .transform(withoutEmptyGroups);
}

// --- FFmpeg and Streamlink ---

const ffmpegShape = {
  enable_lossless_cutting: boolSetting(),
  binary_path: textSetting({ literal: true }),
  input_args: listSetting(),
  output_args: listSetting(),
  timeout_secs: intSetting(0, U32_MAX),
  graceful_stop_timeout_secs: intSetting(0, U32_MAX),
  user_agent: textSetting(),
};

export const FfmpegConfigSchema = engineSettings(ffmpegShape);
export type FfmpegConfig = z.infer<typeof FfmpegConfigSchema>;
export const FfmpegConfigOverrideSchema = overrideSettings(ffmpegShape);
export type FfmpegConfigOverride = z.infer<typeof FfmpegConfigOverrideSchema>;

const streamlinkShape = {
  enable_lossless_cutting: boolSetting(),
  binary_path: textSetting({ literal: true }),
  ffmpeg_path: textSetting({ literal: true }),
  quality: textSetting(),
  extra_args: listSetting(),
  graceful_stop_timeout_secs: intSetting(0, U32_MAX),
  // Twitch proxy playlist (ttv-lol)
  twitch_proxy_playlist: textSetting(),
  twitch_proxy_playlist_exclude: textSetting(),
};

export const StreamlinkConfigSchema = engineSettings(streamlinkShape);
export type StreamlinkConfig = z.infer<typeof StreamlinkConfigSchema>;
export const StreamlinkConfigOverrideSchema = overrideSettings(streamlinkShape);
export type StreamlinkConfigOverride = z.infer<
  typeof StreamlinkConfigOverrideSchema
>;

// --- Mesio HLS ---

export const MesioHttpVersionPreferenceSchema = z.enum([
  'auto',
  'http2_only',
  'http1_only',
]);

const MesioDownloaderBaseSchema = group({
  timeout_ms: intSetting(0),
  connect_timeout_ms: intSetting(0),
  read_timeout_ms: intSetting(0),
  write_timeout_ms: intSetting(0),
  follow_redirects: boolSetting(),
  user_agent: textSetting(),
  params: optionalSetting(z.array(z.array(z.string()).length(2))),
  danger_accept_invalid_certs: boolSetting(),
  force_ipv4: boolSetting(),
  force_ipv6: boolSetting(),
  http_version: optionalSetting(MesioHttpVersionPreferenceSchema),
  http2_keep_alive_interval_ms: intSetting(0),
  pool_max_idle_per_host: intSetting(0),
  pool_idle_timeout_ms: intSetting(0),
});

export const MesioHlsVariantSelectionPolicySchema = z.discriminatedUnion(
  'type',
  [
    z.object({ type: z.literal('highest_bitrate') }),
    z.object({ type: z.literal('lowest_bitrate') }),
    z.object({
      type: z.literal('closest_to_bitrate'),
      target_bitrate: policyInt(0),
    }),
    z.object({ type: z.literal('audio_only') }),
    z.object({ type: z.literal('video_only') }),
    z.object({
      type: z.literal('matching_resolution'),
      width: policyInt(0, U32_MAX),
      height: policyInt(0, U32_MAX),
    }),
    z.object({ type: z.literal('custom'), value: z.string() }),
  ],
);

export const MesioGapSkipStrategySchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('wait_indefinitely') }),
  z.object({ type: z.literal('skip_after_count'), count: policyInt(0) }),
  z.object({
    type: z.literal('skip_after_duration'),
    duration_ms: policyInt(0),
  }),
  z.object({
    type: z.literal('skip_after_both'),
    count: policyInt(0),
    duration_ms: policyInt(0),
  }),
]);

export const MesioHlsConfigSchema = group({
  base: optionalSetting(MesioDownloaderBaseSchema),
  playlist_config: optionalSetting(
    group({
      initial_playlist_fetch_timeout_ms: intSetting(0),
      live_refresh_interval_ms: intSetting(0),
      live_max_refresh_retries: intSetting(0, U32_MAX),
      live_refresh_retry_delay_ms: intSetting(0),
      variant_selection_policy: optionalSetting(
        MesioHlsVariantSelectionPolicySchema,
      ),
      adaptive_refresh_enabled: boolSetting(),
      adaptive_refresh_min_interval_ms: intSetting(0),
      adaptive_refresh_max_interval_ms: intSetting(0),
    }),
  ),
  scheduler_config: optionalSetting(
    group({
      download_concurrency: intSetting(1),
      processed_segment_buffer_multiplier: intSetting(0),
    }),
  ),
  fetcher_config: optionalSetting(
    group({
      segment_download_timeout_ms: intSetting(0),
      max_segment_retries: intSetting(0, U32_MAX),
      segment_retry_delay_base_ms: intSetting(0),
      max_segment_retry_delay_ms: intSetting(0),
      key_download_timeout_ms: intSetting(0),
      max_key_retries: intSetting(0, U32_MAX),
      key_retry_delay_base_ms: intSetting(0),
      max_key_retry_delay_ms: intSetting(0),
      progress_emit_min_bytes: intSetting(0),
      progress_emit_min_interval_ms: intSetting(0),
    }),
  ),
  processor_config: optionalSetting(
    group({
      processed_segment_ttl_ms: intSetting(0),
    }),
  ),
  decryption_config: optionalSetting(
    group({
      key_cache_ttl_ms: intSetting(0),
      offload_decryption_to_cpu_pool: boolSetting(),
    }),
  ),
  cache_config: optionalSetting(
    group({
      playlist_ttl_ms: intSetting(0),
      segment_ttl_ms: intSetting(0),
      decryption_key_ttl_ms: intSetting(0),
    }),
  ),
  output_config: optionalSetting(
    group({
      gap_evaluation_interval_ms: intSetting(0),
      max_pending_init_segments: intSetting(0),
      live_max_overall_stall_duration_ms: disableableIntSetting(),
      live_gap_strategy: optionalSetting(MesioGapSkipStrategySchema),
      vod_gap_strategy: optionalSetting(MesioGapSkipStrategySchema),
      vod_segment_timeout_ms: disableableIntSetting(),
      buffer_limits: optionalSetting(
        group({
          max_segments: intSetting(0),
          max_bytes: intSetting(0),
        }),
      ),
      metrics_enabled: boolSetting(),
    }),
  ),
});

// --- Mesio MPEG-DASH ---

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

/** Language tag shape the backend accepts: 1–8 letters or digits per subtag, joined by '-'. */
const LANGUAGE_TAG = /^[A-Za-z0-9]{1,8}(?:-[A-Za-z0-9]{1,8})*$/;

export const MesioDashVideoQualitySchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('highest') }),
  z.object({ type: z.literal('lowest') }),
  z.object({ type: z.literal('max_height'), height: policyInt(1, U32_MAX) }),
  z.object({ type: z.literal('closest_bandwidth'), bandwidth: policyInt(1) }),
]);
export type MesioDashVideoQuality = z.infer<typeof MesioDashVideoQualitySchema>;

export const MesioDashAudioQualitySchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('highest') }),
  z.object({ type: z.literal('lowest') }),
  z.object({ type: z.literal('closest_bandwidth'), bandwidth: policyInt(1) }),
]);
export type MesioDashAudioQuality = z.infer<typeof MesioDashAudioQualitySchema>;

export const MesioDashLiveStartSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('auto') }),
  z.object({ type: z.literal('live_edge') }),
  z.object({ type: z.literal('delay'), delay_ms: policyInt(0) }),
]);
export type MesioDashLiveStart = z.infer<typeof MesioDashLiveStartSchema>;

const MesioDashFieldsSchema = z.object({
  video_quality: optionalSetting(MesioDashVideoQualitySchema),
  audio_quality: optionalSetting(MesioDashAudioQualitySchema),
  audio_language: textSetting().refine(
    (val) => val === undefined || LANGUAGE_TAG.test(val),
    { message: "Use a language tag such as 'en' or 'pt-BR'" },
  ),
  include_video: boolSetting(),
  include_audio: boolSetting(),
  download_concurrency: intSetting(1),
  manifest_timeout_ms: intSetting(1),
  min_refresh_interval_ms: intSetting(1),
  max_refresh_retries: intSetting(0, U32_MAX),
  segment_timeout_ms: intSetting(1),
  max_segment_retries: intSetting(0, U32_MAX),
  live_start: optionalSetting(MesioDashLiveStartSchema),
});
export type MesioDashConfig = z.infer<typeof MesioDashFieldsSchema>;

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

export const MesioDashConfigSchema = MesioDashFieldsSchema.catchall(
  z.json(),
).superRefine(requireATrack);

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

// --- Mesio ---

const mesioShape = {
  buffer_size: intSetting(1),
  fix_flv: boolSetting(),
  fix_hls: boolSetting(),
  fix_dash: boolSetting(),
  flv_fix: optionalSetting(
    group({
      sequence_header_change_mode: optionalSetting(
        z.enum(['crc32', 'semantic_signature']),
      ),
      drop_duplicate_sequence_headers: boolSetting(),
      duplicate_tag_filtering: boolSetting(),
      duplicate_tag_filter_config: optionalSetting(
        group({
          window_capacity_tags: intSetting(1),
          window_capacity_bytes: intSetting(0),
          replay_backjump_threshold_ms: intSetting(0, U32_MAX),
          enable_replay_offset_matching: boolSetting(),
        }),
      ),
    }),
  ),
  hls: optionalSetting(MesioHlsConfigSchema),
  dash: optionalSetting(MesioDashConfigSchema),
};

export const MesioConfigSchema = engineSettings(mesioShape);
export type MesioConfig = z.infer<typeof MesioConfigSchema>;
export const MesioConfigOverrideSchema = overrideSettings(mesioShape);
export type MesioConfigOverride = z.infer<typeof MesioConfigOverrideSchema>;

// --- Engine Override Schemas ---

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
