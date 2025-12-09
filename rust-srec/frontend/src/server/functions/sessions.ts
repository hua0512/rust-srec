import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import { SessionSchema } from '../../api/schemas';
import { z } from 'zod';

export const listSessions = createServerFn({ method: "GET" })
    .inputValidator((d: { streamer_id?: string; active_only?: boolean } = {}) => d)
    .handler(async ({ data }) => {
        const params = new URLSearchParams();
        if (data.streamer_id) params.set('streamer_id', data.streamer_id);
        if (data.active_only !== undefined) params.set('active_only', data.active_only.toString());

        const json = await fetchBackend(`/sessions?${params.toString()}`);
        return z.array(SessionSchema).parse(json);
    });

export const getSession = createServerFn({ method: "GET" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        const json = await fetchBackend(`/sessions/${id}`);
        return SessionSchema.parse(json);
    });
