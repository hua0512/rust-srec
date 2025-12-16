import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
    NotificationChannelSchema,
    CreateChannelRequestSchema,
    UpdateChannelRequestSchema,
    NotificationEventTypeInfoSchema,
} from '../../api/schemas/notifications';
import { z } from 'zod';

export const listEventTypes = createServerFn({ method: "GET" })
    .handler(async () => {
        const json = await fetchBackend('/notifications/event-types');
        return z.array(NotificationEventTypeInfoSchema).parse(json);
    });

export const listChannels = createServerFn({ method: "GET" })
    .handler(async () => {
        const json = await fetchBackend('/notifications/channels');
        return z.array(NotificationChannelSchema).parse(json);
    });

export const getChannel = createServerFn({ method: "GET" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        const json = await fetchBackend(`/notifications/channels/${id}`);
        return NotificationChannelSchema.parse(json);
    });

export const createChannel = createServerFn({ method: "POST" })
    .inputValidator((data: z.infer<typeof CreateChannelRequestSchema>) => data)
    .handler(async ({ data }) => {
        const json = await fetchBackend('/notifications/channels', {
            method: 'POST',
            body: JSON.stringify(data),
        });
        return NotificationChannelSchema.parse(json);
    });

export const updateChannel = createServerFn({ method: "POST" })
    .inputValidator((d: { id: string; data: z.infer<typeof UpdateChannelRequestSchema> }) => d)
    .handler(async ({ data: { id, data } }) => {
        const json = await fetchBackend(`/notifications/channels/${id}`, {
            method: 'PUT',
            body: JSON.stringify(data),
        });
        return NotificationChannelSchema.parse(json);
    });

export const deleteChannel = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/notifications/channels/${id}`, { method: 'DELETE' });
    });

export const getSubscriptions = createServerFn({ method: "GET" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        const json = await fetchBackend(`/notifications/channels/${id}/subscriptions`);
        return z.array(z.string()).parse(json);
    });

export const updateSubscriptions = createServerFn({ method: "POST" })
    .inputValidator((d: { id: string; events: string[] }) => d)
    .handler(async ({ data: { id, events } }) => {
        const json = await fetchBackend(`/notifications/channels/${id}/subscriptions`, {
            method: 'PUT',
            body: JSON.stringify({ events }),
        });
        return z.array(z.string()).parse(json);
    });

export const testChannel = createServerFn({ method: "POST" })
    .inputValidator((id: string) => id)
    .handler(async ({ data: id }) => {
        await fetchBackend(`/notifications/channels/${id}/test`, { method: 'POST' });
    });
