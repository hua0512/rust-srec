import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import { JobPresetSchema } from '../../api/schemas';
import { z } from 'zod';

// Response schema for preset list with categories and pagination
const PresetListResponseSchema = z.object({
    presets: z.array(JobPresetSchema),
    categories: z.array(z.string()),
    total: z.number(),
    limit: z.number(),
    offset: z.number(),
});

export type PresetListResponse = z.infer<typeof PresetListResponseSchema>;

// Filter parameters for job presets
export interface JobPresetFilters {
    category?: string;
    processor?: string;
    search?: string;
    limit?: number;
    offset?: number;
}

export const listJobPresets = createServerFn({ method: "GET" })
    .inputValidator((d: JobPresetFilters = {}) => d)
    .handler(async ({ data }) => {
        const params = new URLSearchParams();
        if (data.category) params.set('category', data.category);
        if (data.processor) params.set('processor', data.processor);
        if (data.search) params.set('search', data.search);
        if (data.limit !== undefined) params.set('limit', data.limit.toString());
        if (data.offset !== undefined) params.set('offset', data.offset.toString());

        const json = await fetchBackend(`/job/presets?${params.toString()}`);
        return PresetListResponseSchema.parse(json);
    });

export const getJobPreset = createServerFn({ method: "GET" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        const json = await fetchBackend(`/job/presets/${id}`);
        return JobPresetSchema.parse(json);
    });

export const createJobPreset = createServerFn({ method: "POST" })
    .inputValidator((d: { id: string; name: string; description?: string; category?: string; processor: string; config: string }) => d)
    .handler(async ({ data }) => {
        const json = await fetchBackend('/job/presets', {
            method: 'POST',
            body: JSON.stringify(data)
        });
        return JobPresetSchema.parse(json);
    });

export const updateJobPreset = createServerFn({ method: "POST" })
    .inputValidator((d: { id: string; name: string; description?: string; category?: string; processor: string; config: string }) => d)
    .handler(async ({ data }) => {
        const { id, ...body } = data;
        const json = await fetchBackend(`/job/presets/${id}`, {
            method: 'PUT',
            body: JSON.stringify(body)
        });
        return JobPresetSchema.parse(json);
    });

export const deleteJobPreset = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/job/presets/${id}`, { method: 'DELETE' });
    });

export const cloneJobPreset = createServerFn({ method: "POST" })
    .inputValidator((d: { id: string; new_name: string }) => d)
    .handler(async ({ data }) => {
        const { id, new_name } = data;
        const json = await fetchBackend(`/job/presets/${id}/clone`, {
            method: 'POST',
            body: JSON.stringify({ new_name })
        });
        return JobPresetSchema.parse(json);
    });
