import { z } from 'zod';

// --- Engine-Specific Config Schemas ---

export const FfmpegConfigSchema = z.object({
  binary_path: z.string().default('ffmpeg'),
  input_args: z.array(z.string()).default([]),
  output_args: z.array(z.string()).default([]),
  timeout_secs: z.coerce.number().default(30),
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
});
export type StreamlinkConfig = z.infer<typeof StreamlinkConfigSchema>;

export const MesioConfigSchema = z.object({
  buffer_size: z.coerce.number().default(8388608), // 8MB
  fix_flv: z.boolean().default(true),
  fix_hls: z.boolean().default(true),
});
export type MesioConfig = z.infer<typeof MesioConfigSchema>;

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
export const UpdateEngineRequestSchema = z.object({
  name: z.string().min(1, 'Name is required').optional(),
  engine_type: EngineTypeSchema.optional(),
  config: z.record(z.string(), z.unknown()).optional(),
  version: z.string().optional(),
});
export type UpdateEngineRequest = z.infer<typeof UpdateEngineRequestSchema>;

// --- Test Response Schema ---
export const EngineTestResponseSchema = z.object({
  available: z.boolean(),
  version: z.string().optional(),
});
export type EngineTestResponse = z.infer<typeof EngineTestResponseSchema>;
