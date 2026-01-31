import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
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
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/notifications/channels/${id}`);
    return NotificationChannelSchema.parse(json);
  });

export const createChannel = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof CreateChannelRequestSchema>) => data)
  .handler(async ({ data }) => {
    const json = await fetchBackend('/notifications/channels', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return NotificationChannelSchema.parse(json);
  });

export const updateChannel = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: z.infer<typeof UpdateChannelRequestSchema> }) => d,
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(`/notifications/channels/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return NotificationChannelSchema.parse(json);
  });

export const deleteChannel = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/notifications/channels/${id}`, { method: 'DELETE' });
  });

export const getSubscriptions = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      `/notifications/channels/${id}/subscriptions`,
    );
    return z.array(z.string()).parse(json);
  });

export const updateSubscriptions = createServerFn({ method: 'POST' })
  .inputValidator((d: { id: string; events: string[] }) => d)
  .handler(async ({ data: { id, events } }) => {
    const json = await fetchBackend(
      `/notifications/channels/${id}/subscriptions`,
      {
        method: 'PUT',
        body: JSON.stringify({ events }),
      },
    );
    return z.array(z.string()).parse(json);
  });

export const testChannel = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/notifications/channels/${id}/test`, {
      method: 'POST',
    });
  });

export const listEvents = createServerFn({ method: 'GET' })
  .inputValidator(
    (
      q: {
        limit?: number;
        offset?: number;
        event_type?: string;
        streamer_id?: string;
        search?: string;
        priority?: string;
      } = {},
    ) => q,
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    if (data.limit) params.set('limit', String(data.limit));
    if (data.offset) params.set('offset', String(data.offset));
    if (data.event_type) params.set('event_type', data.event_type);
    if (data.streamer_id) params.set('streamer_id', data.streamer_id);
    if (data.search) params.set('search', data.search);
    if (data.priority) params.set('priority', data.priority);

    const qs = params.toString();
    const json = await fetchBackend(
      `/notifications/events${qs ? `?${qs}` : ''}`,
    );
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

export const subscribeWebPush = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: {
      subscription: z.infer<typeof WebPushSubscriptionJsonSchema>;
      min_priority?: string;
    }) => d,
  )
  .handler(async ({ data }) => {
    const payload = {
      subscription: WebPushSubscriptionJsonSchema.parse(data.subscription),
      min_priority: data.min_priority,
    };
    const json = await fetchBackend('/notifications/web-push/subscribe', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    return WebPushSubscriptionSchema.parse(json);
  });

export const unsubscribeWebPush = createServerFn({ method: 'POST' })
  .inputValidator((d: { endpoint: string }) => d)
  .handler(async ({ data }) => {
    await fetchBackend('/notifications/web-push/unsubscribe', {
      method: 'POST',
      body: JSON.stringify({ endpoint: data.endpoint }),
    });
  });
