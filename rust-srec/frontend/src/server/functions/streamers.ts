import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  StreamerSchema,
  CreateStreamerSchema,
  UpdateStreamerSchema,
  ExtractMetadataResponseSchema,
  PrioritySchema,
  BatchStreamerRequestSchema,
  BatchStreamerResponseSchema,
} from '../../api/schemas';
import { z } from 'zod';
import { removeEmpty } from '@/lib/format';
import type { BatchStreamerAction } from '@/api/schemas';

const StreamerFiltersSchema = z.object({
  page: z.number().optional(),
  limit: z.number().optional(),
  search: z.string().optional(),
  platform: z.string().optional(),
  template: z.string().optional(),
  templateUnassigned: z.boolean().optional(),
  state: z.string().optional(),
  priority: z.enum(['HIGH', 'NORMAL', 'LOW']).optional(),
  sortBy: z.enum(['name', 'priority', 'state', 'updated_at']).optional(),
  sortDir: z.enum(['asc', 'desc']).optional(),
});

// `PrioritySchema` carries a schema-level default, and Zod applies a default
// through `.optional()`. Keeping it here would turn a partial update such as the
// enable toggle into a priority overwrite, because the backend treats a present
// `priority` as the new value.
const StreamerUpdateSchema = UpdateStreamerSchema.extend({
  priority: PrioritySchema.removeDefault().optional(),
});

export const listStreamers = createServerFn({ method: 'GET' })
  .validator(
    (
      d: {
        page?: number;
        limit?: number;
        search?: string;
        platform?: string;
        template?: string;
        templateUnassigned?: boolean;
        state?: string;
        priority?: 'HIGH' | 'NORMAL' | 'LOW';
        sortBy?: 'name' | 'priority' | 'state' | 'updated_at';
        sortDir?: 'asc' | 'desc';
      } = {},
    ) => parseInput(StreamerFiltersSchema, d),
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
    if (data.template) params.set('template', data.template);
    if (data.templateUnassigned) params.set('template_unassigned', 'true');
    if (data.state) params.set('state', data.state);
    if (data.priority) params.set('priority', data.priority);
    if (data.sortBy) params.set('sort_by', data.sortBy);
    if (data.sortDir) params.set('sort_dir', data.sortDir);

    const json = await fetchBackend(`/streamers?${params.toString()}`);

    const PaginatedStreamerSchema = z.object({
      items: z.array(StreamerSchema),
      total: z.number(),
      limit: z.number(),
      offset: z.number(),
    });
    return PaginatedStreamerSchema.parse(json);
  });

export const batchUpdateStreamers = createServerFn({ method: 'POST' })
  .validator((data: { ids: string[]; action: BatchStreamerAction }) =>
    parseInput(BatchStreamerRequestSchema, data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/streamers/batch', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BatchStreamerResponseSchema.parse(json);
  });

export const getStreamer = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/streamers/${id}`);
    return StreamerSchema.parse(json);
  });

export const createStreamer = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof CreateStreamerSchema>) =>
    parseInput(CreateStreamerSchema, data),
  )
  .handler(async ({ data }) => {
    const payload = {
      ...data,
      streamer_specific_config: data.streamer_specific_config
        ? removeEmpty(data.streamer_specific_config)
        : undefined,
    };
    const json = await fetchBackend('/streamers', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    return StreamerSchema.parse(json);
  });

export const updateStreamer = createServerFn({ method: 'POST' }) // Using POST to support non-GET, commonly patch is used but server fn usually distinguishes mainly GET/POST
  .validator(
    (d: { id: string; data: z.infer<typeof UpdateStreamerSchema> }) => ({
      id: parseInput(PathIdSchema, d.id),
      data: parseInput(StreamerUpdateSchema, d.data),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    const payload = {
      ...data,
      streamer_specific_config: data.streamer_specific_config
        ? removeEmpty(data.streamer_specific_config)
        : undefined,
    };
    const json = await fetchBackend(backendPath`/streamers/${id}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
    return StreamerSchema.parse(json);
  });

export const deleteStreamer = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/streamers/${id}`, { method: 'DELETE' });
  });

export const checkStreamer = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/streamers/${id}/check`, { method: 'POST' });
  });

export const extractMetadata = createServerFn({ method: 'POST' })
  .validator((url: string) => parseInput(z.url(), url))
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
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/streamers/${id}/clear-error`, {
      method: 'POST',
    });
    return StreamerSchema.parse(json);
  });

/**
 * Update streamer priority.
 * PATCH /api/streamers/{id}/priority
 */
export const updateStreamerPriority = createServerFn({ method: 'POST' })
  .validator((d: { id: string; priority: z.infer<typeof PrioritySchema> }) => ({
    id: parseInput(PathIdSchema, d.id),
    priority: parseInput(PrioritySchema, d.priority),
  }))
  .handler(async ({ data: { id, priority } }) => {
    const json = await fetchBackend(backendPath`/streamers/${id}/priority`, {
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
  .validator((d: { id: string; limit?: number }) => ({
    id: parseInput(PathIdSchema, d.id),
    limit: parseInput(z.number().optional(), d.limit),
  }))
  .handler(async ({ data: { id, limit } }) => {
    const params = new URLSearchParams();
    if (typeof limit === 'number') params.set('limit', String(limit));
    const json = await fetchBackend(
      withQuery(backendPath`/streamers/${id}/check-history`, params),
    );
    return StreamerCheckHistoryResponseSchema.parse(json);
  });
