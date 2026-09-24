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
  BatchResponseSchema,
} from '../../api/schemas';
import { z } from 'zod';
import { removeEmpty } from '@/lib/format';
import type { BatchStreamerAction } from '@/api/schemas';
import { StreamerCheckHistoryEntrySchema } from '@/api/schemas/check-history';

export {
  StreamerCheckHistoryEntrySchema,
  type StreamerCheckHistoryEntry,
} from '@/api/schemas/check-history';

const StreamerFiltersSchema = z.object({
  page: z.number().optional(),
  limit: z.number().optional(),
  search: z.string().optional(),
  platform: z.string().optional(),
  template: z.string().optional(),
  templateUnassigned: z.boolean().optional(),
  state: z.string().optional(),
  priority: PrioritySchema.removeDefault().optional(),
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

    const json = await fetchBackend(withQuery('/streamers', params));

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
    return BatchResponseSchema.parse(json);
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

export const updateStreamer = createServerFn({ method: 'POST' })
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

export const extractMetadata = createServerFn({ method: 'POST' })
  .validator((url: string) => parseInput(z.url(), url))
  .handler(async ({ data: url }) => {
    const json = await fetchBackend('/streamers/extract-metadata', {
      method: 'POST',
      body: JSON.stringify({ url }),
    });
    return ExtractMetadataResponseSchema.parse(json);
  });

const StreamerCheckHistoryResponseSchema = z.object({
  items: z.array(StreamerCheckHistoryEntrySchema),
});

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
