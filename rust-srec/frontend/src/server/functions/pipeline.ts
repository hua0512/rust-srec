import { createServerFn } from '@/server/createServerFn';
import { parseInput, validateWith } from '../validate';
import { fetchBackend, LONG_BACKEND_TIMEOUT_MS } from '../api';
import {
  backendPath,
  PathIdSchema,
  PageSchema,
  withQuery,
  setPagination,
} from '../backend-path';
import {
  paginated,
  JobSchema,
  JobLogsResponseSchema,
  PipelineStatsSchema,
  MediaOutputSchema,
  MediaOutputSummarySchema,
  JobProgressSnapshotSchema,
  StalledOperationListSchema,
  UploadRecordListSchema,
  DagExecutionSchema,
  DagListResponseSchema,
  PipelinePresetSchema,
  PipelinePresetListResponseSchema,
  DagPipelineDefinitionSchema,
  CreatePipelinePresetRequestSchema,
  UpdatePipelinePresetRequestSchema,
  BatchResponseSchema,
  BatchDagRequestSchema,
  BatchDeleteOutputsRequestSchema,
  DeleteOutputResponseSchema,
  InterruptedOutputListSchema,
  DeleteInterruptedOutputResponseSchema,
  BatchDeleteInterruptedOutputsRequestSchema,
} from '../../api/schemas';
import type { DagPipelineDefinition } from '../../api/schemas';
import { z } from 'zod';

const CreatePipelineJobRequestSchema = z.object({
  session_id: z.string().min(1),
  streamer_id: z.string().min(1),
  input_paths: z.array(z.string()).min(1),
  dag: DagPipelineDefinitionSchema,
});

/** Offset pagination shared by the list endpoints. */
const JobLogFiltersSchema = PageSchema.extend({ id: PathIdSchema });

export const getPipelineJobLogs = createServerFn({ method: 'GET' })
  .validator(validateWith(JobLogFiltersSchema))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    setPagination(params, data);

    const path = backendPath`/pipeline/jobs/${data.id}/logs`;
    const json = await fetchBackend(withQuery(path, params));
    return JobLogsResponseSchema.parse(json);
  });

export const getPipelineJobProgress = createServerFn({ method: 'GET' })
  .validator((d: { id: string }) => ({ id: parseInput(PathIdSchema, d.id) }))
  .handler(async ({ data }) => {
    const json = await fetchBackend(
      backendPath`/pipeline/jobs/${data.id}/progress`,
    );
    return JobProgressSnapshotSchema.parse(json);
  });

export const getPipelineJobUploads = createServerFn({ method: 'GET' })
  .validator((d: { id: string }) => ({ id: parseInput(PathIdSchema, d.id) }))
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
  .validator(validateWith(DagFiltersSchema.default({})))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.status) params.set('status', data.status);
    if (data.streamer_id) params.set('streamer_id', data.streamer_id);
    if (data.session_id) params.set('session_id', data.session_id);
    if (data.search) params.set('search', data.search);
    setPagination(params, data);

    const json = await fetchBackend(withQuery('/pipeline/dags', params));
    return DagListResponseSchema.parse(json);
  });

export const getDagExecution = createServerFn({ method: 'GET' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/dag/${id}`);
    return DagExecutionSchema.parse(json);
  });

export const cancelDag = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
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
  .validator(validateWith(PathIdSchema))
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
    const json = await fetchBackend('/pipeline/dags/stats');
    return PipelineStatsSchema.parse(json);
  },
);

/** Ended jobs whose file operation still holds a worker, longest-stalled first. */
export const listStalledOperations = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/pipeline/stalled-operations');
    return StalledOperationListSchema.parse(json).items;
  },
);

export const retryPipelineJob = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/jobs/${id}/retry`, {
      method: 'POST',
    });
  });

export const cancelActivePipelineJob = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/jobs/${id}/cancel`, {
      method: 'POST',
    });
  });

export const deletePipelineJob = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/jobs/${id}`, {
      method: 'DELETE',
    });
  });

export const deletePipeline = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
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
  .validator(validateWith(BatchDagRequestSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/pipeline/dags/batch', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BatchResponseSchema.parse(json);
  });

export const deletePipelineOutput = createServerFn({ method: 'POST' })
  .validator(
    validateWith(z.object({ id: PathIdSchema, deleteFile: z.boolean() })),
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
  .validator(validateWith(BatchDeleteOutputsRequestSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/pipeline/outputs/batch-delete', {
      method: 'POST',
      timeoutMs: LONG_BACKEND_TIMEOUT_MS,
      body: JSON.stringify(data),
    });
    return BatchResponseSchema.parse(json);
  });

/** Partial files interrupted jobs left behind, optionally for one job. */
export const listInterruptedOutputs = createServerFn({ method: 'GET' })
  .validator(validateWith(z.object({ jobId: PathIdSchema.optional() })))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.jobId) params.set('job_id', data.jobId);
    const json = await fetchBackend(
      withQuery('/pipeline/interrupted-outputs', params),
    );
    return InterruptedOutputListSchema.parse(json);
  });

/** Delete one partial file, or keep it, and forget its entry. */
export const deleteInterruptedOutput = createServerFn({ method: 'POST' })
  .validator(
    validateWith(z.object({ id: PathIdSchema, keepFile: z.boolean() })),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend(
      withQuery(
        backendPath`/pipeline/interrupted-outputs/${data.id}`,
        new URLSearchParams({ keep_file: String(data.keepFile) }),
      ),
      { method: 'DELETE' },
    );
    return DeleteInterruptedOutputResponseSchema.parse(json);
  });

export const batchDeleteInterruptedOutputs = createServerFn({ method: 'POST' })
  .validator(validateWith(BatchDeleteInterruptedOutputsRequestSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend(
      '/pipeline/interrupted-outputs/batch-delete',
      {
        method: 'POST',
        body: JSON.stringify(data),
        timeoutMs: LONG_BACKEND_TIMEOUT_MS,
      },
    );
    return BatchResponseSchema.parse(json);
  });

export const createPipelineJob = createServerFn({ method: 'POST' })
  .validator(validateWith(CreatePipelineJobRequestSchema))
  .handler(async ({ data }) => {
    await fetchBackend('/pipeline/create', {
      method: 'POST',
      body: JSON.stringify(data),
    });
  });

export const getPipelineJob = createServerFn({ method: 'GET' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/jobs/${id}`);
    return JobSchema.parse(json);
  });

const PipelineOutputFiltersSchema = z.object({
  session_id: z.string().optional(),
  /** A `MediaFileType` value such as `VIDEO`, matching `MediaOutput.format`. */
  file_type: z.string().optional(),
  search: z.string().optional(),
});

function pipelineOutputFilterParams(
  filters: z.output<typeof PipelineOutputFiltersSchema>,
): URLSearchParams {
  const params = new URLSearchParams();
  if (filters.session_id) params.set('session_id', filters.session_id);
  if (filters.file_type) params.set('file_type', filters.file_type);
  if (filters.search) params.set('search', filters.search);
  return params;
}

export const listPipelineOutputs = createServerFn({ method: 'GET' })
  .validator(
    validateWith(
      PageSchema.extend(PipelineOutputFiltersSchema.shape).default({}),
    ),
  )
  .handler(async ({ data }) => {
    const params = pipelineOutputFilterParams(data);
    setPagination(params, data);

    const json = await fetchBackend(withQuery('/pipeline/outputs', params));
    return paginated(MediaOutputSchema).parse(json);
  });

/**
 * Totals for the outputs matching `filters`, broken down by file type.
 *
 * `file_type` is not accepted: the endpoint always reports every type so the
 * counts behind a type picker stay put while one type is selected.
 */
export const getPipelineOutputSummary = createServerFn({ method: 'GET' })
  .validator(
    validateWith(
      PipelineOutputFiltersSchema.omit({ file_type: true }).default({}),
    ),
  )
  .handler(async ({ data }) => {
    const params = pipelineOutputFilterParams(data);
    const json = await fetchBackend(
      withQuery('/pipeline/outputs/summary', params),
    );
    return MediaOutputSummarySchema.parse(json);
  });

const PipelinePresetFiltersSchema = PageSchema.extend({
  search: z.string().optional(),
});

export const listPipelinePresets = createServerFn({ method: 'GET' })
  .validator(validateWith(PipelinePresetFiltersSchema.default({})))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.search) params.set('search', data.search);
    setPagination(params, data);

    const json = await fetchBackend(withQuery('/pipeline/presets', params));
    return PipelinePresetListResponseSchema.parse(json);
  });

export const getPipelinePreset = createServerFn({ method: 'GET' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/pipeline/presets/${id}`);
    return PipelinePresetSchema.parse(json);
  });

export const createPipelinePreset = createServerFn({ method: 'POST' })
  .validator(validateWith(CreatePipelinePresetRequestSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/pipeline/presets', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return PipelinePresetSchema.parse(json);
  });

export const updatePipelinePreset = createServerFn({ method: 'POST' })
  .validator(
    (d: {
      id: string;
      data: z.input<typeof UpdatePipelinePresetRequestSchema>;
    }) => ({
      id: parseInput(PathIdSchema, d.id),
      data: parseInput(UpdatePipelinePresetRequestSchema, d.data),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(backendPath`/pipeline/presets/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return PipelinePresetSchema.parse(json);
  });

export const deletePipelinePreset = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/pipeline/presets/${id}`, {
      method: 'DELETE',
    });
  });
