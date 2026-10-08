import { z } from 'zod';
import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  CredentialProfileCreateSchema,
  CredentialProfileUpdateSchema,
  CredentialProfileDetailSchema,
  CredentialProfileSummarySchema,
  CredentialAttentionSchema,
  CredentialLoginTargetSchema,
  CredentialLoginGeneratedSchema,
  CredentialLoginReceiptSchema,
  EffectiveCredentialSelectionSchema,
  PlatformCredentialCapabilitiesSchema,
} from '@/api/schemas/credential-profiles';

const ScopeQuerySchema = z.object({
  scope_type: z.enum(['platform', 'template', 'streamer']),
  scope_id: PathIdSchema,
  platform_id: PathIdSchema,
});
export const getEffectiveCredentialSelection = createServerFn({ method: 'GET' })
  .validator((input: z.infer<typeof ScopeQuerySchema>) =>
    parseInput(ScopeQuerySchema, input),
  )
  .handler(async ({ data }) =>
    EffectiveCredentialSelectionSchema.parse(
      await fetchBackend(
        withQuery('/credentials/selection', new URLSearchParams(data)),
      ),
    ),
  );
const PlatformQuerySchema = z.object({ platform_id: PathIdSchema });
/** Lists a platform's accounts, which every scope on the platform shares. */
export const listCredentialProfiles = createServerFn({ method: 'GET' })
  .validator((input: z.infer<typeof PlatformQuerySchema>) =>
    parseInput(PlatformQuerySchema, input),
  )
  .handler(async ({ data }) => {
    const result = await fetchBackend(
      withQuery('/credentials/profiles', new URLSearchParams(data)),
    );
    return z.array(CredentialProfileDetailSchema).parse(result);
  });
/** Enabled accounts on every platform that need the user. */
export const listCredentialAttention = createServerFn({
  method: 'GET',
}).handler(async () =>
  z
    .array(CredentialAttentionSchema)
    .parse(await fetchBackend('/credentials/attention')),
);
/** What the platform's accounts accept and support. */
export const getCredentialCapabilities = createServerFn({ method: 'GET' })
  .validator((input: z.infer<typeof PlatformQuerySchema>) =>
    parseInput(PlatformQuerySchema, input),
  )
  .handler(async ({ data }) =>
    PlatformCredentialCapabilitiesSchema.parse(
      await fetchBackend(
        withQuery('/credentials/capabilities', new URLSearchParams(data)),
      ),
    ),
  );
export const createCredentialProfile = createServerFn({ method: 'POST' })
  .validator((input: z.infer<typeof CredentialProfileCreateSchema>) =>
    parseInput(CredentialProfileCreateSchema, input),
  )
  .handler(async ({ data }) =>
    CredentialProfileSummarySchema.parse(
      await fetchBackend('/credentials/profiles', {
        method: 'POST',
        body: JSON.stringify(data),
      }),
    ),
  );
export const updateCredentialProfile = createServerFn({ method: 'POST' })
  .validator((input: z.infer<typeof CredentialProfileUpdateSchema>) =>
    parseInput(CredentialProfileUpdateSchema, input),
  )
  .handler(async ({ data: { id, ...body } }) =>
    CredentialProfileSummarySchema.parse(
      await fetchBackend(backendPath`/credentials/profiles/${id}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    ),
  );
const DeleteSchema = z.object({
  id: PathIdSchema,
  expected_version: z.number().int().positive(),
});
export const deleteCredentialProfile = createServerFn({ method: 'POST' })
  .validator((input: z.infer<typeof DeleteSchema>) =>
    parseInput(DeleteSchema, input),
  )
  .handler(async ({ data: { id, expected_version } }) => {
    await fetchBackend(backendPath`/credentials/profiles/${id}`, {
      method: 'DELETE',
      body: JSON.stringify({ expected_version }),
    });
  });
export const validateCredentialProfile = createServerFn({ method: 'POST' })
  .validator((input: string) => parseInput(PathIdSchema, input))
  .handler(async ({ data }) =>
    CredentialProfileDetailSchema.parse(
      await fetchBackend(backendPath`/credentials/profiles/${data}/validate`, {
        method: 'POST',
      }),
    ),
  );
export const refreshCredentialProfile = createServerFn({ method: 'POST' })
  .validator((input: string) => parseInput(PathIdSchema, input))
  .handler(async ({ data }) =>
    CredentialProfileDetailSchema.parse(
      await fetchBackend(backendPath`/credentials/profiles/${data}/refresh`, {
        method: 'POST',
      }),
    ),
  );

export const generateCredentialLogin = createServerFn({ method: 'POST' })
  .validator((input: z.infer<typeof CredentialLoginTargetSchema>) =>
    parseInput(CredentialLoginTargetSchema, input),
  )
  .handler(async ({ data }) =>
    CredentialLoginGeneratedSchema.parse(
      await fetchBackend('/credentials/login-sessions', {
        method: 'POST',
        body: JSON.stringify(data),
      }),
    ),
  );
export const pollCredentialLogin = createServerFn({ method: 'POST' })
  .validator((input: string) => parseInput(PathIdSchema, input))
  .handler(async ({ data }) =>
    CredentialLoginReceiptSchema.parse(
      await fetchBackend(
        backendPath`/credentials/login-sessions/${data}/poll`,
        { method: 'POST' },
      ),
    ),
  );
