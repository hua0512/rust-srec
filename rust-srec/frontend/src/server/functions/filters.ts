import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
    FilterSchema,
    CreateFilterRequestSchema,
    UpdateFilterRequestSchema
} from '../../api/schemas';
import { z } from 'zod';

export const listFilters = createServerFn({ method: "GET" })
    .inputValidator((streamerId: string) => streamerId)
    .handler(async ({ data: streamerId }) => {
        const json = await fetchBackend(`/streamers/${streamerId}/filters`);
        return z.array(FilterSchema).parse(json);
    });

export const createFilter = createServerFn({ method: "POST" })
    .inputValidator((d: { streamerId: string; data: z.infer<typeof CreateFilterRequestSchema> }) => d)
    .handler(async ({ data: { streamerId, data } }) => {
        const json = await fetchBackend(`/streamers/${streamerId}/filters`, {
            method: 'POST',
            body: JSON.stringify(data)
        });
        return FilterSchema.parse(json);
    });

export const updateFilter = createServerFn({ method: "POST" })
    .inputValidator((d: { streamerId: string; filterId: string; data: z.infer<typeof UpdateFilterRequestSchema> }) => d)
    .handler(async ({ data: { streamerId, filterId, data } }) => {
        const json = await fetchBackend(`/streamers/${streamerId}/filters/${filterId}`, {
            method: 'PATCH',
            body: JSON.stringify(data)
        });
        return FilterSchema.parse(json);
    });

export const deleteFilter = createServerFn({ method: "POST" })
    .inputValidator((d: { streamerId: string; filterId: string }) => d)
    .handler(async ({ data: { streamerId, filterId } }) => {
        await fetchBackend(`/streamers/${streamerId}/filters/${filterId}`, { method: 'DELETE' });
    });
