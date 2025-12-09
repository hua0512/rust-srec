import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
    JobSchema,
    PipelineStatsSchema,
    MediaOutputSchema
} from '../../api/schemas';
import { z } from 'zod';

export const listPipelineJobs = createServerFn({ method: "GET" })
    .inputValidator((d: { status?: string } = {}) => d)
    .handler(async ({ data }) => {
        const params = new URLSearchParams();
        if (data.status) params.set('status', data.status);

        const json = await fetchBackend(`/pipeline/jobs?${params.toString()}`);
        return z.array(JobSchema).parse(json);
    });

export const getPipelineStats = createServerFn({ method: "GET" })
    .handler(async () => {
        const json = await fetchBackend('/pipeline/stats');
        return PipelineStatsSchema.parse(json);
    });

export const retryPipelineJob = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/pipeline/jobs/${id}/retry`, { method: 'POST' });
    });

export const cancelPipelineJob = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/pipeline/jobs/${id}`, { method: 'DELETE' });
    });

export const createPipelineJob = createServerFn({ method: "POST" })
    .inputValidator((d: { session_id: string }) => d)
    .handler(async ({ data }) => {
        await fetchBackend('/pipeline/create', {
            method: 'POST',
            body: JSON.stringify(data)
        });
    });

export const listPipelineOutputs = createServerFn({ method: "GET" })
    .inputValidator((d: { session_id?: string } = {}) => d)
    .handler(async ({ data }) => {
        const params = new URLSearchParams();
        if (data.session_id) params.set('session_id', data.session_id);

        const json = await fetchBackend(`/pipeline/outputs?${params.toString()}`);
        return z.array(MediaOutputSchema).parse(json);
    });
