import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { SessionDanmuStatisticsSchema, SessionSchema } from '../../api/schemas';
import { z } from 'zod';

const PaginatedSessionSchema = z.object({
  items: z.array(SessionSchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});

export const listSessions = createServerFn({ method: 'GET' })
  .inputValidator(
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
    ) => d,
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

    const json = await fetchBackend(`/sessions?${params.toString()}`);

    return PaginatedSessionSchema.parse(json);
  });

export const getSession = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/sessions/${id}`);
    return SessionSchema.parse(json);
  });

export const getSessionDanmuStatistics = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/sessions/${id}/danmu-statistics`);
    return SessionDanmuStatisticsSchema.parse(json);
  });

export const deleteSession = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/sessions/${id}`, {
      method: 'DELETE',
    });
  });

export const deleteSessions = createServerFn({ method: 'POST' })
  .inputValidator((ids: string[]) => ids)
  .handler(async ({ data: ids }) => {
    const json = await fetchBackend('/sessions/batch-delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ids }),
    });
    return json as { deleted: number };
  });
