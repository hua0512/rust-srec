import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { BASE_URL } from '../../utils/env';
import {
  LoggingConfigResponseSchema,
  UpdateLogFilterRequestSchema,
  LogFilesResponseSchema,
  ArchiveTokenResponseSchema,
} from '../../api/schemas';
import { z } from 'zod';

// --- Logging Configuration ---

/** Get current logging configuration */
export const getLoggingConfig = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/logging');
    return LoggingConfigResponseSchema.parse(json);
  },
);

/** Update logging filter directive */
export const updateLoggingFilter = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof UpdateLogFilterRequestSchema>) => data)
  .handler(async ({ data }) => {
    if (!data || !data.filter) {
      throw new Error('Missing filter in request');
    }
    const json = await fetchBackend('/logging', {
      method: 'PUT',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(data),
    });
    return LoggingConfigResponseSchema.parse(json);
  });

// --- Log Files ---

/** List log files with optional date range filtering */
export const listLogFiles = createServerFn({ method: 'GET' })
  .inputValidator(
    (data: { from?: string; to?: string; limit?: number; offset?: number }) =>
      data,
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data?.from) params.set('from', data.from);
    if (data?.to) params.set('to', data.to);
    if (data?.limit) params.set('limit', String(data.limit));
    if (data?.offset) params.set('offset', String(data.offset));

    const query = params.toString();
    const endpoint = query ? `/logging/files?${query}` : '/logging/files';
    const json = await fetchBackend(endpoint);
    return LogFilesResponseSchema.parse(json);
  });

/** Get archive token for downloading logs */
export const getArchiveToken = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/logging/archive-token');
    return ArchiveTokenResponseSchema.parse(json);
  },
);

/** Build an authenticated download URL for system logs with optional date range. */
export const getLogsDownloadUrl = createServerFn({ method: 'GET' })
  .inputValidator((data?: { from?: string; to?: string }) => data)
  .handler(async ({ data }) => {
    // Ask the backend for a single-use archive token, then build an absolute
    // download URL using the configured API base.
    const json = await fetchBackend('/logging/archive-token');
    const parsed = ArchiveTokenResponseSchema.parse(json);

    const base = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
    const url = new URL(`${base}/logging/archive`);
    url.searchParams.set('token', parsed.token);
    if (data?.from) url.searchParams.set('from', data.from);
    if (data?.to) url.searchParams.set('to', data.to);

    return { url: url.toString(), expires_at: parsed.expires_at };
  });
