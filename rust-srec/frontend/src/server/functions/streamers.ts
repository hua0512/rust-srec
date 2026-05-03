import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import {
  StreamerSchema,
  CreateStreamerSchema,
  UpdateStreamerSchema,
  ExtractMetadataResponseSchema,
  PrioritySchema,
} from '../../api/schemas';
import { z } from 'zod';
import { removeEmpty } from '@/lib/format';

export const listStreamers = createServerFn({ method: 'GET' })
  .inputValidator(
    (
      d: {
        page?: number;
        limit?: number;
        search?: string;
        platform?: string;
        state?: string;
      } = {},
    ) => d,
  )
  .handler(async ({ data }) => {
    // Backend endpoint expects query params with offset-based pagination
    const params = new URLSearchParams();
    // Convert page-based pagination to offset-based
    const limit = data.limit ?? 20;
    if (data.page && data.page > 1) {
      const offset = (data.page - 1) * limit;
      params.set('offset', offset.toString());
    }
    params.set('limit', limit.toString());
    if (data.search) params.set('search', data.search);
    if (data.platform) params.set('platform', data.platform);
    if (data.state) params.set('state', data.state);

    const json = await fetchBackend(`/streamers?${params.toString()}`);

    const PaginatedStreamerSchema = z.object({
      items: z.array(StreamerSchema),
      total: z.number(),
      limit: z.number(),
      offset: z.number(),
    });
    return PaginatedStreamerSchema.parse(json);
  });

export const getStreamer = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/streamers/${id}`);
    return StreamerSchema.parse(json);
  });

export const createStreamer = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof CreateStreamerSchema>) => data)
  .handler(async ({ data }) => {
    const payload = {
      ...data,
      streamer_specific_config: data.streamer_specific_config
        ? removeEmpty(data.streamer_specific_config)
        : undefined,
    };
    console.log('[createStreamer] Payload:', JSON.stringify(payload, null, 2));
    const json = await fetchBackend('/streamers', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    return StreamerSchema.parse(json);
  });

export const updateStreamer = createServerFn({ method: 'POST' }) // Using POST to support non-GET, commonly patch is used but server fn usually distinguishes mainly GET/POST
  .inputValidator(
    (d: { id: string; data: z.infer<typeof UpdateStreamerSchema> }) => d,
  )
  .handler(async ({ data: { id, data } }) => {
    const payload = {
      ...data,
      streamer_specific_config: data.streamer_specific_config
        ? removeEmpty(data.streamer_specific_config)
        : undefined,
    };
    console.log('[updateStreamer] ID:', id);
    console.log('[updateStreamer] Payload:', JSON.stringify(payload, null, 2));
    const json = await fetchBackend(`/streamers/${id}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
    return StreamerSchema.parse(json);
  });

export const deleteStreamer = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/streamers/${id}`, { method: 'DELETE' });
  });

export const checkStreamer = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/streamers/${id}/check`, { method: 'POST' });
  });

export const extractMetadata = createServerFn({ method: 'POST' })
  .inputValidator((url: string) => url)
  .handler(async ({ data: url }) => {
    const json = await fetchBackend('/streamers/extract-metadata', {
      method: 'POST',
      body: JSON.stringify({ url }),
    });
    return ExtractMetadataResponseSchema.parse(json);
  });

/**
 * Clear error state for a streamer.
 * POST /api/streamers/{id}/clear-error
 */
export const clearStreamerError = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/streamers/${id}/clear-error`, {
      method: 'POST',
    });
    return StreamerSchema.parse(json);
  });

/**
 * Update streamer priority.
 * PATCH /api/streamers/{id}/priority
 */
export const updateStreamerPriority = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; priority: z.infer<typeof PrioritySchema> }) => d,
  )
  .handler(async ({ data: { id, priority } }) => {
    const json = await fetchBackend(`/streamers/${id}/priority`, {
      method: 'PATCH',
      body: JSON.stringify({ priority }),
    });
    return StreamerSchema.parse(json);
  });

// One row of the streamer's per-poll check history. Mirrors
// `StreamerCheckHistoryEntry` in the Rust API; defensive parsing —
// `stream_selected` and `streams_extracted_detail` are tolerant
// because malformed persisted JSON degrades to `null` server-side and
// we don't want a render to fail on a stray bar.
const SelectedStreamSummarySchema = z.object({
  quality: z.string().optional(),
  stream_format: z.string().optional(),
  media_format: z.string().optional(),
  bitrate: z.number().optional(),
  codec: z.string().optional(),
  fps: z.number().optional(),
});

export const StreamerCheckHistoryEntrySchema = z.object({
  checked_at: z.string(), // ISO datetime
  duration_ms: z.number(),
  outcome: z.enum([
    'live',
    'offline',
    'filtered',
    'transient_error',
    'fatal_error',
  ]),
  fatal_kind: z.string().nullable().optional(),
  filter_reason: z.string().nullable().optional(),
  error_message: z.string().nullable().optional(),
  streams_extracted: z.number(),
  stream_selected: SelectedStreamSummarySchema.nullable().optional(),
  streams_extracted_detail: z
    .array(SelectedStreamSummarySchema)
    .nullable()
    .optional(),
  title: z.string().nullable().optional(),
  category: z.string().nullable().optional(),
  viewer_count: z.number().nullable().optional(),
});

export const StreamerCheckHistoryResponseSchema = z.object({
  items: z.array(StreamerCheckHistoryEntrySchema),
});

export type StreamerCheckHistoryEntry = z.infer<
  typeof StreamerCheckHistoryEntrySchema
>;

/**
 * Get the streamer's check-history strip rows.
 * GET /api/streamers/{id}/check-history?limit=N
 *
 * Server returns oldest-first so the UI renders left → right = past → now.
 */
export const getStreamerCheckHistory = createServerFn({ method: 'GET' })
  .inputValidator((d: { id: string; limit?: number }) => d)
  .handler(async ({ data: { id, limit } }) => {
    const params = new URLSearchParams();
    if (typeof limit === 'number') params.set('limit', String(limit));
    const qs = params.toString();
    const url = `/streamers/${id}/check-history${qs ? `?${qs}` : ''}`;
    const json = await fetchBackend(url);
    return StreamerCheckHistoryResponseSchema.parse(json);
  });
