import { createServerFn } from '@/server/createServerFn';
import { parseInput, validateWith } from '../validate';
import { fetchBackend, PLATFORM_BACKEND_TIMEOUT_MS } from '../api';
import {
  backendPath,
  PageSchema,
  PathIdSchema,
  withQuery,
  setPagination,
} from '../backend-path';
import {
  NotificationChannelSchema,
  CreateChannelRequestSchema,
  UpdateChannelRequestSchema,
  NotificationEventTypeInfoSchema,
  NotificationEventLogSchema,
  WebPushSubscriptionSchema,
} from '../../api/schemas/notifications';
import { z } from 'zod';

// `settings` is a JSON document. The API returns it serialized, which is what
// the shared request schemas describe, but the channel editor submits the
// object it built and the backend accepts either (`settings: serde_json::Value`).
const ChannelSettingsSchema = z.union([
  z.string(),
  z.record(z.string(), z.unknown()),
]);

const CreateChannelSchema = CreateChannelRequestSchema.extend({
  settings: ChannelSettingsSchema,
});

const UpdateChannelSchema = UpdateChannelRequestSchema.extend({
  settings: ChannelSettingsSchema,
});

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

export const createChannel = createServerFn({ method: 'POST' })
  .validator(validateWith(CreateChannelSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/notifications/channels', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return NotificationChannelSchema.parse(json);
  });

export const updateChannel = createServerFn({ method: 'POST' })
  .validator(
    (d: { id: string; data: z.input<typeof UpdateChannelSchema> }) => ({
      id: parseInput(PathIdSchema, d.id),
      data: parseInput(UpdateChannelSchema, d.data),
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
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/notifications/channels/${id}`, {
      method: 'DELETE',
    });
  });

export const getSubscriptions = createServerFn({ method: 'GET' })
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(
      backendPath`/notifications/channels/${id}/subscriptions`,
    );
    return z.array(z.string()).parse(json);
  });

export const updateSubscriptions = createServerFn({ method: 'POST' })
  .validator((d: { id: string; events: string[] }) => ({
    id: parseInput(PathIdSchema, d.id),
    events: parseInput(z.array(z.string().min(1)), d.events),
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
  .validator(validateWith(PathIdSchema))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/notifications/channels/${id}/test`, {
      method: 'POST',
      timeoutMs: PLATFORM_BACKEND_TIMEOUT_MS,
    });
  });

const EventFiltersSchema = PageSchema.extend({
  event_type: z.string().optional(),
  streamer_id: z.string().optional(),
  search: z.string().optional(),
  priority: z.string().optional(),
});

export const listEvents = createServerFn({ method: 'GET' })
  .validator(validateWith(EventFiltersSchema.default({})))
  .handler(async ({ data }) => {
    const params = new URLSearchParams();
    setPagination(params, data);
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
  .validator(validateWith(SubscribeWebPushSchema))
  .handler(async ({ data }) => {
    const json = await fetchBackend('/notifications/web-push/subscribe', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return WebPushSubscriptionSchema.parse(json);
  });

export const unsubscribeWebPush = createServerFn({ method: 'POST' })
  .validator(validateWith(z.object({ endpoint: z.string().min(1) })))
  .handler(async ({ data }) => {
    await fetchBackend('/notifications/web-push/unsubscribe', {
      method: 'POST',
      body: JSON.stringify({ endpoint: data.endpoint }),
    });
  });
