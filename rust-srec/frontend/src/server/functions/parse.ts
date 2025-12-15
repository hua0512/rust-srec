import { fetchBackend } from '../api';
import { createServerFn } from '@tanstack/react-start';

export interface ParseUrlRequest {
    url: string;
    cookies?: string;
}

export interface ParseUrlResponse {
    success: boolean;
    is_live: boolean;
    media_info?: any;
    error?: string;
}

/**
 * Parse a single URL to extract media info
 */
export const parseUrl = createServerFn({ method: 'POST' })
    .inputValidator((data: ParseUrlRequest) => data)
    .handler(async ({ data }: { data: ParseUrlRequest }) => {
        return fetchBackend<ParseUrlResponse>('/parse', {
            method: 'POST',
            body: JSON.stringify(data),
        });
    });

/**
 * Parse multiple URLs in batch
 */
export const parseUrlBatch = createServerFn({ method: 'POST' })
    .inputValidator((data: ParseUrlRequest[]) => data)
    .handler(async ({ data }: { data: ParseUrlRequest[] }) => {
        return fetchBackend<ParseUrlResponse[]>('/parse/batch', {
            method: 'POST',
            body: JSON.stringify(data),
        });
    });

export interface ResolveUrlRequest {
    url: string;
    stream_info: any;
    cookies?: string;
}

export interface ResolveUrlResponse {
    success: boolean;
    stream_info?: any;
    error?: string;
}

/**
 * Resolve the true URL for a stream
 */
export const resolveUrl = createServerFn({ method: 'POST' })
    .inputValidator((data: ResolveUrlRequest) => data)
    .handler(async ({ data }: { data: ResolveUrlRequest }) => {
        return fetchBackend<ResolveUrlResponse>('/parse/resolve', {
            method: 'POST',
            body: JSON.stringify(data),
        });
    });
