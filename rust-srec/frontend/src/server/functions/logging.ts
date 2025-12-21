import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
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
