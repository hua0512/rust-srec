import { createServerFn } from '@/server/createServerFn';
import { validateWith } from '../validate';
import { fetchBackend } from '../api';
import { PageSchema, withQuery, setPagination } from '../backend-path';
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

// An empty directive is a valid filter that silences every module, so it is
// treated as a missing value rather than forwarded.
const UpdateLoggingFilterSchema = UpdateLogFilterRequestSchema.extend({
  filter: z.string().min(1),
});

/** Update logging filter directive */
export const updateLoggingFilter = createServerFn({ method: 'POST' })
  .validator(validateWith(UpdateLoggingFilterSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/logging', {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return LoggingConfigResponseSchema.parse(json);
  });

// --- Log Files ---

const LogFileFiltersSchema = PageSchema.extend({
  from: z.string().optional(),
  to: z.string().optional(),
});

/** List log files with optional date range filtering */
export const listLogFiles = createServerFn({ method: 'GET' })
  .validator(validateWith(LogFileFiltersSchema.default({})))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.from) params.set('from', data.from);
    if (data.to) params.set('to', data.to);
    setPagination(params, data);

    const json = await fetchBackend(withQuery('/logging/files', params));
    return LogFilesResponseSchema.parse(json);
  });

/** Fetch a single-use token for downloading the system log archive. */
export const getLogsDownloadUrl = createServerFn({ method: 'GET' }).handler(
  async () => {
    // Ask the backend for a single-use archive token
    const json = await fetchBackend('/logging/archive-token');
    return ArchiveTokenResponseSchema.parse(json);
  },
);
