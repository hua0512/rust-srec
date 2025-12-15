import { fetchBackend } from '../api';
import { createServerFn } from '@tanstack/react-start';
import {
    ParseUrlRequestSchema,
    ParseUrlResponseSchema,
    ResolveUrlRequestSchema,
    ResolveUrlResponseSchema,
} from '../../api/schemas';
import { z } from 'zod';

// Re-export types from schemas for convenience
export type ParseUrlRequest = z.infer<typeof ParseUrlRequestSchema>;
export type ParseUrlResponse = z.infer<typeof ParseUrlResponseSchema>;
export type ResolveUrlRequest = z.infer<typeof ResolveUrlRequestSchema>;
export type ResolveUrlResponse = z.infer<typeof ResolveUrlResponseSchema>;

/**
 * Parse a single URL to extract media info
 */
export const parseUrl = createServerFn({ method: 'POST' })
    .inputValidator((data: ParseUrlRequest) => ParseUrlRequestSchema.parse(data))
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
    .inputValidator((data: ParseUrlRequest[]) =>
        z.array(ParseUrlRequestSchema).parse(data),
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
    .inputValidator((data: ResolveUrlRequest) =>
        ResolveUrlRequestSchema.parse(data),
    )
    .handler(async ({ data }: { data: ResolveUrlRequest }) => {
        const json = await fetchBackend('/parse/resolve', {
            method: 'POST',
            body: JSON.stringify(data),
        });
        return ResolveUrlResponseSchema.parse(json);
    });

