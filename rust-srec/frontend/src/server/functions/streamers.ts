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
