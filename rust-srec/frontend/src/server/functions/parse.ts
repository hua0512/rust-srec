import { fetchBackend } from '../api';
import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
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
  .validator((data: ParseUrlRequest) => parseInput(ParseUrlRequestSchema, data))
  .handler(async ({ data }: { data: ParseUrlRequest }) => {
    const json = await fetchBackend('/parse', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return ParseUrlResponseSchema.parse(json);
  });

/**
 * Parse multiple URLs in batch
 */
export const parseUrlBatch = createServerFn({ method: 'POST' })
  .validator((data: ParseUrlRequest[]) =>
    parseInput(z.array(ParseUrlRequestSchema), data),
  )
  .handler(async ({ data }: { data: ParseUrlRequest[] }) => {
    const json = await fetchBackend('/parse/batch', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return z.array(ParseUrlResponseSchema).parse(json);
  });

/**
 * Resolve the true URL for a stream
 */
export const resolveUrl = createServerFn({ method: 'POST' })
  .validator((data: ResolveUrlRequest) =>
    parseInput(ResolveUrlRequestSchema, data),
  )
  .handler(async ({ data }: { data: ResolveUrlRequest }) => {
    const json = await fetchBackend('/parse/resolve', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return ResolveUrlResponseSchema.parse(json);
  });
