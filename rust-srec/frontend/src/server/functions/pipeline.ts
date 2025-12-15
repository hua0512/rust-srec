import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
  JobSchema,
  PipelineStatsSchema,
  MediaOutputSchema,
  PipelineStepSchema,
  JobProgressSnapshotSchema,
} from '../../api/schemas';
import { z } from 'zod';

const CreatePipelineJobRequestSchema = z.object({
  session_id: z.string().min(1),
  streamer_id: z.string().min(1),
  input_path: z.string().min(1),
  steps: z
    .array(PipelineStepSchema)
    .min(1, 'Pipeline requires at least one step'),
});

export type CreatePipelineJobRequest = z.infer<
  typeof CreatePipelineJobRequestSchema
>;

export const listPipelineJobs = createServerFn({ method: 'GET' })
  .inputValidator(
    (
      d: {
        status?: string;
        session_id?: string;
        pipeline_id?: string;
        search?: string;
        limit?: number;
        offset?: number;
      } = {},
    ) => d,
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.status) params.set('status', data.status);
    if (data.session_id) params.set('session_id', data.session_id);
    if (data.pipeline_id) params.set('pipeline_id', data.pipeline_id);
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/pipeline/jobs?${params.toString()}`);
    return z
      .object({
        items: z.array(JobSchema),
        total: z.number(),
        limit: z.number(),
        offset: z.number(),
      })
      .parse(json);
  });

export const listPipelineJobsPage = createServerFn({ method: 'GET' })
  .inputValidator(
    (
      d: {
        status?: string;
        session_id?: string;
        pipeline_id?: string;
        search?: string;
        limit?: number;
        offset?: number;
      } = {},
    ) => d,
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.status) params.set('status', data.status);
    if (data.session_id) params.set('session_id', data.session_id);
    if (data.pipeline_id) params.set('pipeline_id', data.pipeline_id);
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/pipeline/jobs/page?${params.toString()}`);
    return z
      .object({
        items: z.array(JobSchema),
        limit: z.number(),
        offset: z.number(),
      })
      .parse(json);
  });

export const getPipelineJobLogs = createServerFn({ method: 'GET' })
  .inputValidator((d: { id: string; limit?: number; offset?: number }) => d)
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(
      `/pipeline/jobs/${data.id}/logs?${params.toString()}`,
    );
    return z
      .object({
        items: z.array(
          z.object({
            timestamp: z.string(),
            level: z.string(),
            message: z.string(),
          }),
        ),
        total: z.number(),
        limit: z.number(),
        offset: z.number(),
      })
      .parse(json);
  });

export const getPipelineJobProgress = createServerFn({ method: 'GET' })
  .inputValidator((d: { id: string }) => d)
  .handler(async ({ data }) => {
    const json = await fetchBackend(`/pipeline/jobs/${data.id}/progress`);
    return JobProgressSnapshotSchema.parse(json);
  });

// Pipeline summary schema for list_pipelines endpoint
const PipelineSummarySchema = z.object({
  pipeline_id: z.string(),
  streamer_id: z.string(),
  session_id: z.string().nullable().optional(),
  status: z.string(),
  job_count: z.number(),
  completed_count: z.number(),
  failed_count: z.number(),
  total_duration_secs: z.number(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type PipelineSummary = z.infer<typeof PipelineSummarySchema>;

export const listPipelines = createServerFn({ method: 'GET' })
  .inputValidator(
    (
      d: {
        status?: string;
        streamer_id?: string;
        session_id?: string;
        search?: string;
        limit?: number;
        offset?: number;
      } = {},
    ) => d,
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.status) params.set('status', data.status);
    if (data.streamer_id) params.set('streamer_id', data.streamer_id);
    if (data.session_id) params.set('session_id', data.session_id);
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/pipeline/pipelines?${params.toString()}`);
    return z
      .object({
        items: z.array(PipelineSummarySchema),
        total: z.number(),
        limit: z.number(),
        offset: z.number(),
      })
      .parse(json);
  });

export const getPipelineStats = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/pipeline/stats');
    return PipelineStatsSchema.parse(json);
  },
);

export const retryPipelineJob = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/pipeline/jobs/${id}/retry`, { method: 'POST' });
  });

export const cancelPipelineJob = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/pipeline/jobs/${id}`, { method: 'DELETE' });
  });

export const cancelPipeline = createServerFn({ method: 'POST' })
  .inputValidator((pipelineId: string) => pipelineId)
  .handler(async ({ data: pipelineId }) => {
    const json = await fetchBackend(`/pipeline/${pipelineId}`, {
      method: 'DELETE',
    });
    return z
      .object({
        success: z.boolean(),
        message: z.string(),
        cancelled_count: z.number(),
      })
      .parse(json);
  });

export const createPipelineJob = createServerFn({ method: 'POST' })
  .inputValidator((data: CreatePipelineJobRequest) =>
    CreatePipelineJobRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    await fetchBackend('/pipeline/create', {
      method: 'POST',
      body: JSON.stringify(data),
    });
  });

export const getPipelineJob = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/jobs/${id}`);
    return JobSchema.parse(json);
  });

export const listPipelineOutputs = createServerFn({ method: 'GET' })
  .inputValidator(
    (
      d: {
        session_id?: string;
        search?: string;
        limit?: number;
        offset?: number;
      } = {},
    ) => d,
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.session_id) params.set('session_id', data.session_id);
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/pipeline/outputs?${params.toString()}`);
    return z
      .object({
        items: z.array(MediaOutputSchema),
        total: z.number(),
        limit: z.number(),
        offset: z.number(),
      })
      .parse(json);
  });

// Pipeline Preset schema (workflow sequences)
const PipelinePresetSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string().nullable().optional(),
  steps: z.array(PipelineStepSchema),
  created_at: z.string(),
  updated_at: z.string(),
});
export type PipelinePreset = z.infer<typeof PipelinePresetSchema>;

// Response schema for pipeline preset list with pagination
const PipelinePresetListResponseSchema = z.object({
  presets: z.array(PipelinePresetSchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});

export type PipelinePresetListResponse = z.infer<
  typeof PipelinePresetListResponseSchema
>;

// Filter parameters for pipeline presets
export interface PipelinePresetFilters {
  search?: string;
  limit?: number;
  offset?: number;
}

export const listPipelinePresets = createServerFn({ method: 'GET' })
  .inputValidator((d: PipelinePresetFilters = {}) => d)
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/pipeline/presets?${params.toString()}`);
    return PipelinePresetListResponseSchema.parse(json);
  });

export const getPipelinePreset = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/presets/${id}`);
    return PipelinePresetSchema.parse(json);
  });

export const createPipelinePreset = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: {
      name: string;
      description?: string;
      steps: z.infer<typeof PipelineStepSchema>[];
    }) => d,
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/pipeline/presets', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return PipelinePresetSchema.parse(json);
  });

export const updatePipelinePreset = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: {
      id: string;
      name: string;
      description?: string;
      steps: z.infer<typeof PipelineStepSchema>[];
    }) => d,
  )
  .handler(async ({ data }) => {
    const { id, ...body } = data;
    const json = await fetchBackend(`/pipeline/presets/${id}`, {
      method: 'PUT',
      body: JSON.stringify(body),
    });
    return PipelinePresetSchema.parse(json);
  });

export const deletePipelinePreset = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/pipeline/presets/${id}`, { method: 'DELETE' });
  });
