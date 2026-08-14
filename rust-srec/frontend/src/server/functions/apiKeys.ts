import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { z } from 'zod';

export const ApiKeyAccessLevelSchema = z.enum(['read_only', 'full']);
export type ApiKeyAccessLevel = z.infer<typeof ApiKeyAccessLevelSchema>;

export const ApiKeySchema = z.object({
  id: z.string(),
  name: z.string(),
  key_prefix: z.string(),
  access_level: ApiKeyAccessLevelSchema,
  expires_at: z.number().nullable().optional(),
  last_used_at: z.number().nullable().optional(),
  created_at: z.number(),
  revoked_at: z.number().nullable().optional(),
});
export type ApiKey = z.infer<typeof ApiKeySchema>;

export const CreateApiKeyResponseSchema = z.object({
  api_key: z.string(),
  key: ApiKeySchema,
});
export type CreateApiKeyResponse = z.infer<typeof CreateApiKeyResponseSchema>;

const CreateApiKeyInputSchema = z.object({
  name: z.string().min(1).max(100),
  access_level: ApiKeyAccessLevelSchema,
  expires_at: z.number().nullable().optional(),
});

export const listApiKeys = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/auth/api-keys');
    return z.array(ApiKeySchema).parse(json);
  },
);

export const createApiKey = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof CreateApiKeyInputSchema>) =>
    CreateApiKeyInputSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/auth/api-keys', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return CreateApiKeyResponseSchema.parse(json);
  });

export const revokeApiKey = createServerFn({ method: 'POST' })
  .validator((id: string) => z.string().parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(`/auth/api-keys/${id}`, { method: 'DELETE' });
  });
