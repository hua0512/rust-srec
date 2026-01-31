import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import {
  JobSchema,
  PipelineStatsSchema,
  MediaOutputSchema,
  JobProgressSnapshotSchema,
  DagExecutionSchema,
  DagGraphSchema,
  DagStatsSchema,
  DagListResponseSchema,
  PipelinePresetSchema,
  PipelinePresetListResponseSchema,
  PipelinePresetPreviewSchema,
  DagPipelineDefinitionSchema,
  CreatePipelinePresetRequestSchema,
  UpdatePipelinePresetRequestSchema,
} from '../../api/schemas';
import { z } from 'zod';

const CreatePipelineJobRequestSchema = z.object({
  session_id: z.string().min(1),
  streamer_id: z.string().min(1),
  input_paths: z.array(z.string()).min(1),
  dag: DagPipelineDefinitionSchema,
});

export type CreatePipelineJobRequest = z.infer<
  typeof CreatePipelineJobRequestSchema
>;

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

// DagSummary is used for list_pipelines results
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

    const json = await fetchBackend(`/pipeline/dags?${params.toString()}`);
    return DagListResponseSchema.parse(json);
  });

export const getDagExecution = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/dag/${id}`);
    return DagExecutionSchema.parse(json);
  });

export const getDagGraph = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/dag/${id}/graph`);
    return DagGraphSchema.parse(json);
  });

export const getDagStats = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/dag/${id}/stats`);
    return DagStatsSchema.parse(json);
  });

export const cancelDag = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/dag/${id}`, {
      method: 'DELETE',
    });
    return z
      .object({
        dag_id: z.string(),
        cancelled_steps: z.number(),
        message: z.string(),
      })
      .parse(json);
  });

export const retryDagSteps = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/dag/${id}/retry`, {
      method: 'POST',
    });
    return z
      .object({
        dag_id: z.string(),
        retried_steps: z.number(),
        job_ids: z.array(z.string()),
        message: z.string(),
      })
      .parse(json);
  });

export const retryAllFailedPipelines = createServerFn({
  method: 'POST',
}).handler(async () => {
  const json = await fetchBackend('/pipeline/dags/retry_failed', {
    method: 'POST',
  });
  return z
    .object({
      success: z.boolean(),
      count: z.number(),
      message: z.string(),
    })
    .parse(json);
});

export const validateDagDefinition = createServerFn({ method: 'POST' })
  .inputValidator((dag: any) => dag)
  .handler(async ({ data: dag }) => {
    const json = await fetchBackend('/pipeline/validate', {
      method: 'POST',
      body: JSON.stringify({ dag }),
    });
    return z
      .object({
        valid: z.boolean(),
        errors: z.array(z.string()),
        warnings: z.array(z.string()),
        root_steps: z.array(z.string()),
        leaf_steps: z.array(z.string()),
        max_depth: z.number(),
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
    const json = await fetchBackend(`/pipeline/dag/${pipelineId}`, {
      method: 'DELETE',
    });
    return z
      .object({
        dag_id: z.string(),
        cancelled_steps: z.number(),
        message: z.string(),
      })
      .parse(json);
  });

export const deletePipeline = createServerFn({ method: 'POST' })
  .inputValidator((pipelineId: string) => pipelineId)
  .handler(async ({ data: pipelineId }) => {
    const json = await fetchBackend(`/pipeline/dag/${pipelineId}/delete`, {
      method: 'DELETE',
    });
    return z
      .object({
        dag_id: z.string(),
        message: z.string(),
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

// Redundant schemas removed - now imported from api/schemas
export type PipelinePreset = z.infer<typeof PipelinePresetSchema>;
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
    console.log(
      '[getPipelinePreset] Raw Response:',
      JSON.stringify(json, null, 2),
    );
    return PipelinePresetSchema.parse(json);
  });

export const createPipelinePreset = createServerFn({ method: 'POST' })
  .inputValidator((d: z.infer<typeof CreatePipelinePresetRequestSchema>) => d)
  .handler(async ({ data }) => {
    console.log(
      '[createPipelinePreset] Payload:',
      JSON.stringify(data, null, 2),
    );
    try {
      const json = await fetchBackend('/pipeline/presets', {
        method: 'POST',
        body: JSON.stringify(data),
      });
      console.log(
        '[createPipelinePreset] Raw Response:',
        JSON.stringify(json, null, 2),
      );
      const parsed = PipelinePresetSchema.safeParse(json);
      if (!parsed.success) {
        console.error(
          '[createPipelinePreset] Zod schema validation failed:',
          parsed.error,
        );
        throw new Error('Response validation failed');
      }
      return parsed.data;
    } catch (err) {
      console.error('[createPipelinePreset] Error:', err);
      throw err;
    }
  });

export const updatePipelinePreset = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: {
      id: string;
      data: z.infer<typeof UpdatePipelinePresetRequestSchema>;
    }) => d,
  )
  .handler(async ({ data }) => {
    console.log(
      '[updatePipelinePreset] Payload:',
      JSON.stringify(data, null, 2),
    );
    const { id, data: body } = data;
    try {
      const json = await fetchBackend(`/pipeline/presets/${id}`, {
        method: 'PUT',
        body: JSON.stringify(body),
      });
      console.log(
        '[updatePipelinePreset] Raw Response:',
        JSON.stringify(json, null, 2),
      );
      const parsed = PipelinePresetSchema.safeParse(json);
      console.log(
        '[updatePipelinePreset] Parsed Response:',
        JSON.stringify(parsed, null, 2),
      );
      if (!parsed.success) {
        console.error(
          '[updatePipelinePreset] Zod schema validation failed:',
          parsed.error,
        );
        throw new Error('Response validation failed');
      }
      return parsed.data;
    } catch (err) {
      console.error('[updatePipelinePreset] Error:', err);
      throw err;
    }
  });

export const deletePipelinePreset = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/pipeline/presets/${id}`, { method: 'DELETE' });
  });

export const previewPipelinePreset = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/pipeline/presets/${id}/preview`);
    return PipelinePresetPreviewSchema.parse(json);
  });
