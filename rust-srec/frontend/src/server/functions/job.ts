import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema } from '../backend-path';
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
  // Exact preset name; matches at most one preset. `search` is a substring match over both name
  // and description, so it cannot be used to resolve a preset referenced by name.
  name?: string;
  search?: string;
  limit?: number;
  offset?: number;
}

const JobPresetFiltersSchema = z.object({
  category: z.string().optional(),
  processor: z.string().optional(),
  name: z.string().optional(),
  search: z.string().optional(),
  limit: z.number().optional(),
  offset: z.number().optional(),
});

/** Body shared by the preset create and update endpoints. */
const JobPresetWriteSchema = z.object({
  id: PathIdSchema,
  name: z.string().min(1),
  description: z.string().optional(),
  category: z.string().optional(),
  processor: z.string().min(1),
  config: z.record(z.string(), z.unknown()),
});

interface JobPresetWriteInput {
  id: string;
  name: string;
  description?: string;
  category?: string;
  processor: string;
  config: Record<string, unknown>;
}

export const listJobPresets = createServerFn({ method: 'GET' })
  .validator((d: JobPresetFilters = {}) => JobPresetFiltersSchema.parse(d))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.category) params.set('category', data.category);
    if (data.processor) params.set('processor', data.processor);
    if (data.name) params.set('name', data.name);
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/job/presets?${params.toString()}`);
    return PresetListResponseSchema.parse(json);
  });

export const getJobPreset = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/job/presets/${id}`);
    return JobPresetSchema.parse(json);
  });

export const createJobPreset = createServerFn({ method: 'POST' })
  .validator((d: JobPresetWriteInput) => JobPresetWriteSchema.parse(d))
  .handler(async ({ data }) => {
    // Stringify config before sending to backend
    const payload = {
      ...data,
      config: JSON.stringify(data.config),
    };
    const json = await fetchBackend('/job/presets', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    return JobPresetSchema.parse(json);
  });

export const updateJobPreset = createServerFn({ method: 'POST' })
  .validator((d: JobPresetWriteInput) => JobPresetWriteSchema.parse(d))
  .handler(async ({ data }) => {
    const { id, ...rest } = data;
    // Stringify config before sending to backend
    const body = {
      ...rest,
      config: JSON.stringify(data.config),
    };
    const json = await fetchBackend(backendPath`/job/presets/${id}`, {
      method: 'PUT',
      body: JSON.stringify(body),
    });
    return JobPresetSchema.parse(json);
  });

export const deleteJobPreset = createServerFn({ method: 'POST' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/job/presets/${id}`, { method: 'DELETE' });
  });

export const cloneJobPreset = createServerFn({ method: 'POST' })
  .validator((d: { id: string; new_name: string }) => ({
    id: PathIdSchema.parse(d.id),
    new_name: z.string().min(1).parse(d.new_name),
  }))
  .handler(async ({ data }) => {
    const { id, new_name } = data;
    const json = await fetchBackend(backendPath`/job/presets/${id}/clone`, {
      method: 'POST',
      body: JSON.stringify({ new_name }),
    });
    return JobPresetSchema.parse(json);
  });
