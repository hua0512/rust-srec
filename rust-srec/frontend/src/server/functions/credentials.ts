import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend, BackendApiError } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  CredentialSourceResponseSchema,
  CredentialRefreshResponseSchema,
  QrGenerateResponseSchema,
  QrPollResponseSchema,
} from '../../api/schemas';
import { z } from 'zod';

const TemplateCredentialInputSchema = z.object({
  id: PathIdSchema,
  platform: z.string().min(1).optional(),
});

/** Query for the template endpoints, which scope the lookup by platform. */
function templatePlatformQuery(platform?: string): URLSearchParams {
  const params = new URLSearchParams();
  if (platform) params.set('platform', platform);
  return params;
}

/** The credential source for a scope, or `null` when the scope has none (404). */
async function fetchCredentialSource(path: string) {
  try {
    const json = await fetchBackend(path);
    return CredentialSourceResponseSchema.parse(json);
  } catch (e) {
    if (e instanceof BackendApiError && e.status === 404) {
      return null;
    }
    throw e;
  }
}

export const getStreamerCredentialSource = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    return fetchCredentialSource(
      backendPath`/credentials/streamers/${id}/source`,
    );
  });

export const getPlatformCredentialSource = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    return fetchCredentialSource(
      backendPath`/credentials/platforms/${id}/source`,
    );
  });

export const getTemplateCredentialSource = createServerFn({ method: 'GET' })
  .validator((input: { id: string; platform?: string }) =>
    parseInput(TemplateCredentialInputSchema, input),
  )
  .handler(async ({ data }) => {
    const { id, platform } = data;
    return fetchCredentialSource(
      withQuery(
        backendPath`/credentials/templates/${id}/source`,
        templatePlatformQuery(platform),
      ),
    );
  });

export const refreshStreamerCredentials = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/credentials/streamers/${id}/refresh`,
      {
        method: 'POST',
      },
    );
    return CredentialRefreshResponseSchema.parse(json);
  });

export const refreshPlatformCredentials = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/credentials/platforms/${id}/refresh`,
      {
        method: 'POST',
      },
    );
    return CredentialRefreshResponseSchema.parse(json);
  });

export const refreshTemplateCredentials = createServerFn({ method: 'POST' })
  .validator((input: { id: string; platform?: string }) =>
    parseInput(TemplateCredentialInputSchema, input),
  )
  .handler(async ({ data }) => {
    const { id, platform } = data;
    const json = await fetchBackend(
      withQuery(
        backendPath`/credentials/templates/${id}/refresh`,
        templatePlatformQuery(platform),
      ),
      {
        method: 'POST',
      },
    );
    return CredentialRefreshResponseSchema.parse(json);
  });

// Bilibili QR Login

export const generateBilibiliQr = createServerFn({ method: 'POST' }).handler(
  async () => {
    const json = await fetchBackend('/credentials/bilibili/qr/generate', {
      method: 'POST',
    });
    return QrGenerateResponseSchema.parse(json);
  },
);

export type CredentialSaveScope =
  | { type: 'platform'; id: string }
  | { type: 'template'; id: string }
  | { type: 'streamer'; id: string };

export interface PollBilibiliQrInput {
  auth_code: string;
  scope: CredentialSaveScope;
}

const PollBilibiliQrInputSchema = z.object({
  auth_code: z.string().min(1),
  scope: z.object({
    type: z.enum(['platform', 'template', 'streamer']),
    id: z.string().min(1),
  }),
});

export const pollBilibiliQr = createServerFn({ method: 'POST' })
  .validator((input: PollBilibiliQrInput) =>
    parseInput(PollBilibiliQrInputSchema, input),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/credentials/bilibili/qr/poll', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return QrPollResponseSchema.parse(json);
  });
