import { createServerFn } from '@/server/createServerFn';
import { validateWith } from '../validate';
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
  SessionDanmuStatisticsSchema,
  SessionSchema,
  SessionSegmentSchema,
} from '../../api/schemas';
import { z } from 'zod';

const PaginatedSessionSchema = paginated(SessionSchema);

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
  .validator(validateWith(SessionFiltersSchema.default({})))
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
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/sessions/${id}`);
    return SessionSchema.parse(json);
  });

export const getSessionDanmuStatistics = createServerFn({ method: 'GET' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/sessions/${id}/danmu-statistics`,
    );
    return SessionDanmuStatisticsSchema.parse(json);
  });

export const deleteSession = createServerFn({ method: 'POST' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/sessions/${id}`, {
      method: 'DELETE',
    });
  });

export const deleteSessions = createServerFn({ method: 'POST' })
  .validator(validateWith(z.array(z.string().min(1)).min(1)))
  .handler(async ({ data: ids }) => {
    const json = await fetchBackend('/sessions/batch-delete', {
      method: 'POST',
      timeoutMs: LONG_BACKEND_TIMEOUT_MS,
      body: JSON.stringify({ ids }),
    });
    return json as { deleted: number };
  });

export const listSessionSegments = createServerFn({ method: 'GET' })
  .validator(validateWith(PageSchema.extend({ session_id: PathIdSchema })))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    setPagination(params, data);

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
