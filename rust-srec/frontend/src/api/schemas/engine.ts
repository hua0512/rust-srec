import { z } from 'zod'; // Added import

// --- Engine ---
export const EngineConfigSchema = z.object({
    id: z.string(),
    name: z.string(),
    engine_type: z.string().optional(),
    config: z.json().optional(), // JSON config for the engine
});
export type EngineConfig = z.infer<typeof EngineConfigSchema>;

export const CreateEngineRequestSchema = EngineConfigSchema.omit({ id: true });
export type CreateEngineRequest = z.infer<typeof CreateEngineRequestSchema>;

export const UpdateEngineRequestSchema = CreateEngineRequestSchema.partial().extend(
    {
        version: z.string().optional(),
    },
);


export const EngineTestResponseSchema = z.object({
    available: z.boolean(),
    version: z.string().optional(),
});
export type EngineTestResponse = z.infer<typeof EngineTestResponseSchema>;