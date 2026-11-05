import { fetchBackend, PLATFORM_BACKEND_TIMEOUT_MS } from '../api';
import { createServerFn } from '@/server/createServerFn';
import { validateWith } from '../validate';
import {
  ParseUrlRequestSchema,
  ParseUrlResponseSchema,
  ResolveUrlRequestSchema,
  ResolveUrlResponseSchema,
  type ParseUrlRequest,
  type ResolveUrlRequest,
} from '../../api/schemas';
import { z } from 'zod';

/**
 * Parse a single URL to extract media info
 */
export const parseUrl = createServerFn({ method: 'POST' })
  .validator(validateWith(ParseUrlRequestSchema))
  .handler(async ({ data }: { data: ParseUrlRequest }) => {
    const json = await fetchBackend('/parse', {
      method: 'POST',
      timeoutMs: PLATFORM_BACKEND_TIMEOUT_MS,
      body: JSON.stringify(data),
    });
    return ParseUrlResponseSchema.parse(json);
  });

export const renewPlayback = createServerFn({ method: 'POST' })
  .validator(validateWith(z.string().min(1)))
  .handler(async ({ data }) =>
    ParseUrlResponseSchema.parse(
      await fetchBackend('/parse/playback/renew', {
        method: 'POST',
        timeoutMs: PLATFORM_BACKEND_TIMEOUT_MS,
        body: JSON.stringify({ playback_handle: data }),
      }),
    ),
  );

/**
 * Parse multiple URLs in batch
 */
export const parseUrlBatch = createServerFn({ method: 'POST' })
  .validator(validateWith(z.array(ParseUrlRequestSchema)))
  .handler(async ({ data }: { data: ParseUrlRequest[] }) => {
    const json = await fetchBackend('/parse/batch', {
      method: 'POST',
      timeoutMs: PLATFORM_BACKEND_TIMEOUT_MS,
      body: JSON.stringify(data),
    });
    return z.array(ParseUrlResponseSchema).parse(json);
  });

/**
 * Resolve the true URL for a stream
 */
export const resolveUrl = createServerFn({ method: 'POST' })
  .validator(validateWith(ResolveUrlRequestSchema))
  .handler(async ({ data }: { data: ResolveUrlRequest }) => {
    const json = await fetchBackend('/parse/resolve', {
      method: 'POST',
      timeoutMs: PLATFORM_BACKEND_TIMEOUT_MS,
      body: JSON.stringify(data),
    });
    return ResolveUrlResponseSchema.parse(json);
  });
