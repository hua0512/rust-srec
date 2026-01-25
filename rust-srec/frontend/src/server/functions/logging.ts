import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { BASE_URL } from '../../utils/env';
import {
  LoggingConfigResponseSchema,
  UpdateLogFilterRequestSchema,
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

/** Build an authenticated download URL for system logs. */
export const getLogsDownloadUrl = createServerFn({ method: 'GET' }).handler(
  async () => {
    // Ask the backend for a single-use archive token, then build an absolute
    // download URL using the configured API base.
    const json = await fetchBackend('/logging/archive-token');
    const parsed = z
      .object({ token: z.string().min(1), expires_at: z.string().min(1) })
      .parse(json);

    const base = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
    const url = new URL(`${base}/logging/archive`);
    url.searchParams.set('token', parsed.token);

    return { url: url.toString(), expires_at: parsed.expires_at };
  },
);
