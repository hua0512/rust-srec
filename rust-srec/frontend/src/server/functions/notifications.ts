import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  NotificationChannelSchema,
  CreateChannelRequestSchema,
  UpdateChannelRequestSchema,
  NotificationEventTypeInfoSchema,
  NotificationEventLogSchema,
  WebPushSubscriptionSchema,
} from '../../api/schemas/notifications';
import { z } from 'zod';

export const listEventTypes = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/notifications/event-types');
    return z.array(NotificationEventTypeInfoSchema).parse(json);
  },
);

export const listChannels = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/notifications/channels');
    return z.array(NotificationChannelSchema).parse(json);
  },
);

export const getChannel = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/notifications/channels/${id}`);
    return NotificationChannelSchema.parse(json);
  });

export const createChannel = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof CreateChannelRequestSchema>) =>
    CreateChannelRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/notifications/channels', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return NotificationChannelSchema.parse(json);
  });

export const updateChannel = createServerFn({ method: 'POST' })
  .validator(
    (d: { id: string; data: z.infer<typeof UpdateChannelRequestSchema> }) => ({
      id: PathIdSchema.parse(d.id),
      data: UpdateChannelRequestSchema.parse(d.data),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(
      backendPath`/notifications/channels/${id}`,
      {
        method: 'PUT',
        body: JSON.stringify(data),
      },
    );
    return NotificationChannelSchema.parse(json);
  });

export const deleteChannel = createServerFn({ method: 'POST' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/notifications/channels/${id}`, {
      method: 'DELETE',
    });
  });

export const getSubscriptions = createServerFn({ method: 'GET' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/notifications/channels/${id}/subscriptions`,
    );
    return z.array(z.string()).parse(json);
  });

export const updateSubscriptions = createServerFn({ method: 'POST' })
  .validator((d: { id: string; events: string[] }) => ({
    id: PathIdSchema.parse(d.id),
    events: z.array(z.string().min(1)).parse(d.events),
  }))
  .handler(async ({ data: { id, events } }) => {
    const json = await fetchBackend(
      backendPath`/notifications/channels/${id}/subscriptions`,
      {
        method: 'PUT',
        body: JSON.stringify({ events }),
      },
    );
    return z.array(z.string()).parse(json);
  });

export const testChannel = createServerFn({ method: 'POST' })
  .validator((id: string) => PathIdSchema.parse(id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/notifications/channels/${id}/test`, {
      method: 'POST',
    });
  });

const EventFiltersSchema = z.object({
  limit: z.number().optional(),
  offset: z.number().optional(),
  event_type: z.string().optional(),
  streamer_id: z.string().optional(),
  search: z.string().optional(),
  priority: z.string().optional(),
});

export const listEvents = createServerFn({ method: 'GET' })
  .validator(
    (
      q: {
        limit?: number;
        offset?: number;
        event_type?: string;
        streamer_id?: string;
        search?: string;
        priority?: string;
      } = {},
    ) => EventFiltersSchema.parse(q),
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.limit) params.set('limit', String(data.limit));
    if (data.offset) params.set('offset', String(data.offset));
    if (data.event_type) params.set('event_type', data.event_type);
    if (data.streamer_id) params.set('streamer_id', data.streamer_id);
    if (data.search) params.set('search', data.search);
    if (data.priority) params.set('priority', data.priority);

    const json = await fetchBackend(withQuery('/notifications/events', params));
    return z.array(NotificationEventLogSchema).parse(json);
  });

// --- Web Push (VAPID) ---

const WebPushPublicKeySchema = z.object({
  public_key: z.string(),
});

export const getWebPushPublicKey = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/notifications/web-push/public-key');
    return WebPushPublicKeySchema.parse(json);
  },
);

export const listWebPushSubscriptions = createServerFn({
  method: 'GET',
}).handler(async () => {
  const json = await fetchBackend('/notifications/web-push/subscriptions');
  return z.array(WebPushSubscriptionSchema).parse(json);
});

const WebPushSubscriptionJsonSchema = z.object({
  endpoint: z.string().url(),
  keys: z.object({
    p256dh: z.string().min(1),
    auth: z.string().min(1),
  }),
});

const SubscribeWebPushSchema = z.object({
  subscription: WebPushSubscriptionJsonSchema,
  min_priority: z.number().optional(),
});

export const subscribeWebPush = createServerFn({ method: 'POST' })
  .validator(
    (d: {
      subscription: z.infer<typeof WebPushSubscriptionJsonSchema>;
      min_priority?: number;
    }) => SubscribeWebPushSchema.parse(d),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/notifications/web-push/subscribe', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return WebPushSubscriptionSchema.parse(json);
  });

export const unsubscribeWebPush = createServerFn({ method: 'POST' })
  .validator((d: { endpoint: string }) =>
    z.object({ endpoint: z.string().min(1) }).parse(d),
  )
  .handler(async ({ data }) => {
    await fetchBackend('/notifications/web-push/unsubscribe', {
      method: 'POST',
      body: JSON.stringify({ endpoint: data.endpoint }),
    });
  });
