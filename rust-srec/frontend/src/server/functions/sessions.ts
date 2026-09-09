import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  SessionDanmuStatisticsSchema,
  SessionSchema,
  SessionSegmentSchema,
} from '../../api/schemas';
import { z } from 'zod';

const PaginatedSessionSchema = z.object({
  items: z.array(SessionSchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});

const SessionFiltersSchema = z.object({
  page: z.number().optional(),
  limit: z.number().optional(),
  streamer_id: z.string().optional(),
  active_only: z.boolean().optional(),
  from_date: z.string().optional(),
  to_date: z.string().optional(),
  search: z.string().optional(),
});

export const listSessions = createServerFn({ method: 'GET' })
  .validator(
    (
      d: {
        page?: number;
        limit?: number;
        streamer_id?: string;
        active_only?: boolean;
        from_date?: string;
        to_date?: string;
        search?: string;
      } = {},
    ) => parseInput(SessionFiltersSchema, d),
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    const page = data.page || 1;
    const limit = data.limit || 20;
    const offset = (page - 1) * limit;

    params.set('limit', limit.toString());
    params.set('offset', offset.toString());

    if (data.streamer_id) params.set('streamer_id', data.streamer_id);
    if (data.active_only !== undefined)
      params.set('active_only', data.active_only.toString());
    if (data.from_date) params.set('from_date', data.from_date);
    if (data.to_date) params.set('to_date', data.to_date);
    if (data.search) params.set('search', data.search);

    const json = await fetchBackend(withQuery('/sessions', params));

    return PaginatedSessionSchema.parse(json);
  });

export const getSession = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/sessions/${id}`);
    return SessionSchema.parse(json);
  });

export const getSessionDanmuStatistics = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/sessions/${id}/danmu-statistics`,
    );
    return SessionDanmuStatisticsSchema.parse(json);
  });

export const deleteSession = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/sessions/${id}`, {
      method: 'DELETE',
    });
  });

export const deleteSessions = createServerFn({ method: 'POST' })
  .validator((ids: string[]) =>
    parseInput(z.array(z.string().min(1)).min(1), ids),
  )
  .handler(async ({ data: ids }) => {
    const json = await fetchBackend('/sessions/batch-delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ids }),
    });
    return json as { deleted: number };
  });

export const listSessionSegments = createServerFn({ method: 'GET' })
  .validator((d: { session_id: string; limit?: number; offset?: number }) => ({
    session_id: parseInput(PathIdSchema, d.session_id),
    limit: parseInput(z.number().optional(), d.limit),
    offset: parseInput(z.number().optional(), d.offset),
  }))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.limit !== undefined) params.set('limit', data.limit.toString());
    if (data.offset !== undefined) params.set('offset', data.offset.toString());

    const path = backendPath`/sessions/${data.session_id}/segments`;
    const json = await fetchBackend(withQuery(path, params));

    return z
      .object({
        items: z.array(SessionSegmentSchema),
        limit: z.number(),
        offset: z.number(),
      })
      .parse(json);
  });
