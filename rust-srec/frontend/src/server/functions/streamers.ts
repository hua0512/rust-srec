import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
    StreamerSchema,
    CreateStreamerSchema,
    UpdateStreamerSchema,
    ExtractMetadataResponseSchema
} from '../../api/schemas';
import { z } from 'zod';

export const listStreamers = createServerFn({ method: "GET" })
    .inputValidator((d: { page?: number; limit?: number; search?: string; platform?: string; state?: string } = {}) => d)
    .handler(async ({ data }) => {
        // Backend endpoint expects query params
        const params = new URLSearchParams();
        if (data.page) params.set('page', data.page.toString());
        if (data.limit) params.set('limit', data.limit.toString());
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

export const getStreamer = createServerFn({ method: "GET" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        const json = await fetchBackend(`/streamers/${id}`);
        return StreamerSchema.parse(json);
    });

export const createStreamer = createServerFn({ method: "POST" })
    .inputValidator((data: z.infer<typeof CreateStreamerSchema>) => data)
    .handler(async ({ data }) => {
        const json = await fetchBackend('/streamers', {
            method: 'POST',
            body: JSON.stringify(data)
        });
        return StreamerSchema.parse(json);
    });

export const updateStreamer = createServerFn({ method: "POST" }) // Using POST to support non-GET, commonly patch is used but server fn usually distinguishes mainly GET/POST
    .inputValidator((d: { id: string; data: z.infer<typeof UpdateStreamerSchema> }) => d)
    .handler(async ({ data: { id, data } }) => {
        const json = await fetchBackend(`/streamers/${id}`, {
            method: 'PATCH',
            body: JSON.stringify(data)
        });
        return StreamerSchema.parse(json);
    });

export const deleteStreamer = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/streamers/${id}`, { method: 'DELETE' });
    });

export const checkStreamer = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/streamers/${id}/check`, { method: 'POST' });
    });

export const extractMetadata = createServerFn({ method: "POST" })
    .inputValidator((url: string) => url)
    .handler(async ({ data: url }) => {
        const json = await fetchBackend('/streamers/extract-metadata', {
            method: 'POST',
            body: JSON.stringify({ url })
        });
        return ExtractMetadataResponseSchema.parse(json);
    });
