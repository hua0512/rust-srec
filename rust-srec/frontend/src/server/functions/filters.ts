import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema } from '../backend-path';
import {
  FilterSchema,
  CreateFilterRequestSchema,
  UpdateFilterRequestSchema,
} from '../../api/schemas';
import { z } from 'zod';

export const listFilters = createServerFn({ method: 'GET' })
  .validator((streamerId: string) => parseInput(PathIdSchema, streamerId))
  .handler(async ({ data: streamerId }) => {
    const json = await fetchBackend(
      backendPath`/streamers/${streamerId}/filters`,
    );
    return z.array(FilterSchema).parse(json);
  });

export const createFilter = createServerFn({ method: 'POST' })
  .validator(
    (d: {
      streamerId: string;
      data: z.infer<typeof CreateFilterRequestSchema>;
    }) => ({
      streamerId: parseInput(PathIdSchema, d.streamerId),
      data: parseInput(CreateFilterRequestSchema, d.data),
    }),
  )
  .handler(async ({ data: { streamerId, data } }) => {
    const json = await fetchBackend(
      backendPath`/streamers/${streamerId}/filters`,
      {
        method: 'POST',
        body: JSON.stringify({ ...data, streamer_id: streamerId }),
      },
    );
    return FilterSchema.parse(json);
  });

export const updateFilter = createServerFn({ method: 'POST' })
  .validator(
    (d: {
      streamerId: string;
      filterId: string;
      data: z.infer<typeof UpdateFilterRequestSchema>;
    }) => ({
      streamerId: parseInput(PathIdSchema, d.streamerId),
      filterId: parseInput(PathIdSchema, d.filterId),
      data: parseInput(UpdateFilterRequestSchema, d.data),
    }),
  )
  .handler(async ({ data: { streamerId, filterId, data } }) => {
    const json = await fetchBackend(
      backendPath`/streamers/${streamerId}/filters/${filterId}`,
      {
        method: 'PATCH',
        body: JSON.stringify(data),
      },
    );
    return FilterSchema.parse(json);
  });

export const deleteFilter = createServerFn({ method: 'POST' })
  .validator((d: { streamerId: string; filterId: string }) => ({
    streamerId: parseInput(PathIdSchema, d.streamerId),
    filterId: parseInput(PathIdSchema, d.filterId),
  }))
  .handler(async ({ data: { streamerId, filterId } }) => {
    await fetchBackend(
      backendPath`/streamers/${streamerId}/filters/${filterId}`,
      {
        method: 'DELETE',
      },
    );
  });
