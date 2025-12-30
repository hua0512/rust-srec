import { createServerFn } from '@tanstack/react-start';
import { fetchBackend, BackendApiError } from '../api';
import {
  CredentialSourceResponseSchema,
  CredentialRefreshResponseSchema,
  QrGenerateResponseSchema,
  QrPollResponseSchema,
} from '../../api/schemas';

export const getStreamerCredentialSource = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    try {
      const json = await fetchBackend(`/credentials/streamers/${id}/source`);
      return CredentialSourceResponseSchema.parse(json);
    } catch (e) {
      if (e instanceof BackendApiError && e.status === 404) {
        return null;
      }
      throw e;
    }
  });

export const getPlatformCredentialSource = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    try {
      const json = await fetchBackend(`/credentials/platforms/${id}/source`);
      return CredentialSourceResponseSchema.parse(json);
    } catch (e) {
      if (e instanceof BackendApiError && e.status === 404) {
        return null;
      }
      throw e;
    }
  });

export const getTemplateCredentialSource = createServerFn({ method: 'GET' })
  .inputValidator((input: { id: string; platform?: string }) => input)
  .handler(async ({ data }) => {
    const { id, platform } = data;
    try {
      const qs = platform ? `?platform=${encodeURIComponent(platform)}` : '';
      const json = await fetchBackend(
        `/credentials/templates/${id}/source${qs}`,
      );
      return CredentialSourceResponseSchema.parse(json);
    } catch (e) {
      if (e instanceof BackendApiError && e.status === 404) {
        return null;
      }
      throw e;
    }
  });

export const refreshStreamerCredentials = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/credentials/streamers/${id}/refresh`, {
      method: 'POST',
    });
    return CredentialRefreshResponseSchema.parse(json);
  });

export const refreshPlatformCredentials = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/credentials/platforms/${id}/refresh`, {
      method: 'POST',
    });
    return CredentialRefreshResponseSchema.parse(json);
  });

export const refreshTemplateCredentials = createServerFn({ method: 'POST' })
  .inputValidator((input: { id: string; platform?: string }) => input)
  .handler(async ({ data }) => {
    const { id, platform } = data;
    const qs = platform ? `?platform=${encodeURIComponent(platform)}` : '';
    const json = await fetchBackend(
      `/credentials/templates/${id}/refresh${qs}`,
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

export const pollBilibiliQr = createServerFn({ method: 'POST' })
  .inputValidator((input: PollBilibiliQrInput) => input)
  .handler(async ({ data }) => {
    const json = await fetchBackend('/credentials/bilibili/qr/poll', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return QrPollResponseSchema.parse(json);
  });
