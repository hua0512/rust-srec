import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  JobSchema,
  PipelineStatsSchema,
  MediaOutputSchema,
  MediaOutputSummarySchema,
  JobProgressSnapshotSchema,
  UploadRecordListSchema,
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
  BatchResponseSchema,
  BatchDagRequestSchema,
  BatchDeleteOutputsRequestSchema,
  DeleteOutputResponseSchema,
} from '../../api/schemas';
import type {
  BatchDagRequest,
  BatchDeleteOutputsRequest,
  DagPipelineDefinition,
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

/** Offset pagination shared by the list endpoints. */
const PageSchema = z.object({
  limit: z.number().optional(),
  offset: z.number().optional(),
});

const JobLogFiltersSchema = PageSchema.extend({ id: PathIdSchema });

export const getPipelineJobLogs = createServerFn({ method: 'GET' })
  .validator((d: { id: string; limit?: number; offset?: number }) =>
    JobLogFiltersSchema.parse(d),
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const path = backendPath`/pipeline/jobs/${data.id}/logs`;
    const json = await fetchBackend(`${path}?${params.toString()}`);
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
  .validator((d: { id: string }) => ({ id: PathIdSchema.parse(d.id) }))
  .handler(async ({ data }) => {
    const json = await fetchBackend(
      backendPath`/pipeline/jobs/${data.id}/progress`,
    );
    return JobProgressSnapshotSchema.parse(json);
  });

export const getPipelineJobUploads = createServerFn({ method: 'GET' })
  .validator((d: { id: string }) => ({ id: PathIdSchema.parse(d.id) }))
  .handler(async ({ data }) => {
    const json = await fetchBackend(
      backendPath`/pipeline/jobs/${data.id}/uploads`,
    );
    return UploadRecordListSchema.parse(json);
  });

const DagFiltersSchema = PageSchema.extend({
  status: z.string().optional(),
  streamer_id: z.string().optional(),
  session_id: z.string().optional(),
  search: z.string().optional(),
});

// DagSummary is used for list_pipelines results
export const listPipelines = createServerFn({ method: 'GET' })
  .validator(
    (
      d: {
        status?: string;
        streamer_id?: string;
        session_id?: string;
        search?: string;
        limit?: number;
        offset?: number;
      } = {},
    ) => DagFiltersSchema.parse(d),
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
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${id}`);
    return DagExecutionSchema.parse(json);
  });

export const getDagGraph = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${id}/graph`);
    return DagGraphSchema.parse(json);
  });

export const getDagStats = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${id}/stats`);
    return DagStatsSchema.parse(json);
  });

export const cancelDag = createServerFn({ method: 'POST' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${id}`, {
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
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${id}/retry`, {
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

// Type-only validator, deliberately not `DagPipelineDefinitionSchema.parse`:
// this endpoint exists to have the backend explain what is wrong with a DAG,
// so rejecting the payload here would swallow the report the caller wants.
export const validateDagDefinition = createServerFn({ method: 'POST' })
  .validator((dag: DagPipelineDefinition) => dag)
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
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/jobs/${id}/retry`, {
      method: 'POST',
    });
  });

export const cancelActivePipelineJob = createServerFn({ method: 'POST' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/jobs/${id}/cancel`, {
      method: 'POST',
    });
  });

export const deletePipelineJob = createServerFn({ method: 'POST' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/jobs/${id}`, {
      method: 'DELETE',
    });
  });

export const cancelPipeline = createServerFn({ method: 'POST' })
  .validator((pipelineId: string) => PathIdSchema.parse(pipelineId))
  .handler(async ({ data: pipelineId }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${pipelineId}`, {
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
  .validator((pipelineId: string) => PathIdSchema.parse(pipelineId))
  .handler(async ({ data: pipelineId }) => {
    const json = await fetchBackend(
      backendPath`/pipeline/dag/${pipelineId}/delete`,
      {
        method: 'DELETE',
      },
    );
    return z
      .object({
        dag_id: z.string(),
        message: z.string(),
      })
      .parse(json);
  });

export const batchPipelines = createServerFn({ method: 'POST' })
  .validator((data: BatchDagRequest) => BatchDagRequestSchema.parse(data))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/pipeline/dags/batch', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BatchResponseSchema.parse(json);
  });

export const deletePipelineOutput = createServerFn({ method: 'POST' })
  .validator((data: { id: string; deleteFile: boolean }) =>
    z.object({ id: PathIdSchema, deleteFile: z.boolean() }).parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend(
      withQuery(
        backendPath`/pipeline/outputs/${data.id}`,
        new URLSearchParams({ delete_file: String(data.deleteFile) }),
      ),
      { method: 'DELETE' },
    );
    return DeleteOutputResponseSchema.parse(json);
  });

export const batchDeletePipelineOutputs = createServerFn({ method: 'POST' })
  .validator((data: BatchDeleteOutputsRequest) =>
    BatchDeleteOutputsRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/pipeline/outputs/batch-delete', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BatchResponseSchema.parse(json);
  });

export const createPipelineJob = createServerFn({ method: 'POST' })
  .validator((data: CreatePipelineJobRequest) =>
    CreatePipelineJobRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    await fetchBackend('/pipeline/create', {
      method: 'POST',
      body: JSON.stringify(data),
    });
  });

export const getPipelineJob = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/jobs/${id}`);
    return JobSchema.parse(json);
  });

interface PipelineOutputFilters {
  session_id?: string;
  /** A `MediaFileType` value such as `VIDEO`, matching `MediaOutput.format`. */
  file_type?: string;
  search?: string;
}

const PipelineOutputFiltersSchema = z.object({
  session_id: z.string().optional(),
  file_type: z.string().optional(),
  search: z.string().optional(),
});

function pipelineOutputFilterParams(
  filters: PipelineOutputFilters,
): URLSearchParams {
  const params = new URLSearchParams();
  if (filters.session_id) params.set('session_id', filters.session_id);
  if (filters.file_type) params.set('file_type', filters.file_type);
  if (filters.search) params.set('search', filters.search);
  return params;
}

export const listPipelineOutputs = createServerFn({ method: 'GET' })
  .validator(
    (
      d: PipelineOutputFilters & {
        limit?: number;
        offset?: number;
      } = {},
    ) => PageSchema.extend(PipelineOutputFiltersSchema.shape).parse(d),
  )
  .handler(async ({ data }) => {
    const params = pipelineOutputFilterParams(data);
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

/**
 * Totals for the outputs matching `filters`, broken down by file type.
 *
 * `file_type` is not accepted: the endpoint always reports every type so the
 * counts behind a type picker stay put while one type is selected.
 */
export const getPipelineOutputSummary = createServerFn({ method: 'GET' })
  .validator((d: Omit<PipelineOutputFilters, 'file_type'> = {}) =>
    PipelineOutputFiltersSchema.omit({ file_type: true }).parse(d),
  )
  .handler(async ({ data }) => {
    const params = pipelineOutputFilterParams(data);
    const json = await fetchBackend(
      `/pipeline/outputs/summary?${params.toString()}`,
    );
    return MediaOutputSummarySchema.parse(json);
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

const PipelinePresetFiltersSchema = PageSchema.extend({
  search: z.string().optional(),
});

export const listPipelinePresets = createServerFn({ method: 'GET' })
  .validator((d: PipelinePresetFilters = {}) =>
    PipelinePresetFiltersSchema.parse(d),
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.search) params.set('search', data.search);
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const json = await fetchBackend(`/pipeline/presets?${params.toString()}`);
    return PipelinePresetListResponseSchema.parse(json);
  });

export const getPipelinePreset = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/presets/${id}`);
    return PipelinePresetSchema.parse(json);
  });

export const createPipelinePreset = createServerFn({ method: 'POST' })
  .validator((d: z.infer<typeof CreatePipelinePresetRequestSchema>) =>
    CreatePipelinePresetRequestSchema.parse(d),
  )
  .handler(async ({ data }) => {
    try {
      const json = await fetchBackend('/pipeline/presets', {
        method: 'POST',
        body: JSON.stringify(data),
      });
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
  .validator(
    (d: {
      id: string;
      data: z.infer<typeof UpdatePipelinePresetRequestSchema>;
    }) => ({
      id: PathIdSchema.parse(d.id),
      data: UpdatePipelinePresetRequestSchema.parse(d.data),
    }),
  )
  .handler(async ({ data }) => {
    const { id, data: body } = data;
    try {
      const json = await fetchBackend(backendPath`/pipeline/presets/${id}`, {
        method: 'PUT',
        body: JSON.stringify(body),
      });
      const parsed = PipelinePresetSchema.safeParse(json);
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
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/presets/${id}`, {
      method: 'DELETE',
    });
  });

export const previewPipelinePreset = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/pipeline/presets/${id}/preview`,
    );
    return PipelinePresetPreviewSchema.parse(json);
  });
