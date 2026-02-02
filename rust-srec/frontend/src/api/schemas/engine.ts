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
  key_download_timeout_ms: optionalInt(0),
  max_key_retries: optionalInt(0),
  key_retry_delay_base_ms: optionalInt(0),
  segment_raw_cache_ttl_ms: optionalInt(0),
  streaming_threshold_bytes: optionalInt(0),
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

export const MesioHlsOutputConfigOverrideSchema = z.object({
  live_reorder_buffer_duration_ms: optionalInt(0),
  live_reorder_buffer_max_segments: optionalInt(0),
  gap_evaluation_interval_ms: optionalInt(0),
  max_pending_init_segments: optionalInt(0),
  live_max_overall_stall_duration_ms: optionalInt(0),
  live_gap_strategy: MesioGapSkipStrategySchema.optional(),
  vod_gap_strategy: MesioGapSkipStrategySchema.optional(),
  vod_segment_timeout_ms: optionalInt(0),
  buffer_limits: MesioBufferLimitsOverrideSchema.optional(),
  metrics_enabled: z.boolean().optional(),
});

export const MesioPrefetchOverrideSchema = z.object({
  enabled: z.boolean().optional(),
  prefetch_count: optionalInt(0),
  max_buffer_before_skip: optionalInt(0),
});

export const MesioBufferPoolOverrideSchema = z.object({
  enabled: z.boolean().optional(),
  pool_size: optionalInt(0),
  default_capacity: optionalInt(0),
});

export const MesioBatchSchedulerOverrideSchema = z.object({
  enabled: z.boolean().optional(),
  batch_window_ms: optionalInt(0),
  max_batch_size: optionalInt(0),
});

export const MesioHlsPerformanceConfigOverrideSchema = z.object({
  decryption_offload_enabled: z.boolean().optional(),
  prefetch: MesioPrefetchOverrideSchema.optional(),
  buffer_pool: MesioBufferPoolOverrideSchema.optional(),
  batch_scheduler: MesioBatchSchedulerOverrideSchema.optional(),
  zero_copy_enabled: z.boolean().optional(),
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
  performance_config: MesioHlsPerformanceConfigOverrideSchema.optional(),
  live_gap_strategy: MesioGapSkipStrategySchema.optional(),
  prefetch: MesioPrefetchOverrideSchema.optional(),
  download_concurrency: optionalInt(1),
});

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
  hls: MesioHlsConfigSchema.optional(),
});
export type MesioConfig = z.infer<typeof MesioConfigSchema>;

// --- Engine Override Schemas ---
// Used by template engine overrides. These are *partial* (no defaults) and
// strict (unknown keys fail), so typos don't silently get ignored.

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
    hls: MesioHlsConfigSchema.optional(),
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
