import { z } from 'zod';

export const ChannelTypeSchema = z.enum(['Discord', 'Email', 'Webhook']);
export type ChannelType = z.infer<typeof ChannelTypeSchema>;

// Settings schemas
export const DiscordSettingsSchema = z.object({
  webhook_url: z.url(),
  username: z.string().optional(),
  avatar_url: z.url().optional(),
  min_priority: z.enum(['Low', 'Normal', 'High', 'Critical']).default('Normal'),
  enabled: z.boolean().default(true),
});

export const EmailSettingsSchema = z.object({
  smtp_host: z.string().min(1),
  smtp_port: z.number().int().positive(),
  username: z.string().min(1),
  password: z.string().min(1),
  from_address: z.email(),
  to_addresses: z.array(z.email()).min(1),
  use_tls: z.boolean().default(true),
  min_priority: z.enum(['Low', 'Normal', 'High', 'Critical']).default('High'),
  enabled: z.boolean().default(true),
});

export const WebhookAuthTypeSchema = z.enum([
  'None',
  'Bearer',
  'Basic',
  'Header',
]);
export type WebhookAuthType = z.infer<typeof WebhookAuthTypeSchema>;

export const WebhookAuthSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('None') }),
  z.object({ type: z.literal('Bearer'), token: z.string() }),
  z.object({
    type: z.literal('Basic'),
    username: z.string(),
    password: z.string(),
  }),
  z.object({ type: z.literal('Header'), name: z.string(), value: z.string() }),
]);

export const WebhookSettingsSchema = z.object({
  url: z.url(),
  headers: z.array(z.tuple([z.string(), z.string()])).optional(),
  method: z.string().default('POST'),
  auth: WebhookAuthSchema.optional(),
  min_priority: z.enum(['Low', 'Normal', 'High', 'Critical']).default('Low'),
  enabled: z.boolean().default(true),
  timeout_secs: z.number().int().positive().default(30),
});

export const NotificationChannelSchema = z.object({
  id: z.uuid(),
  name: z.string().min(1),
  channel_type: ChannelTypeSchema,
  settings: z.string(),
});

export type NotificationChannel = z.infer<typeof NotificationChannelSchema>;

export const CreateChannelRequestSchema = z.object({
  name: z.string().min(1),
  channel_type: ChannelTypeSchema,
  settings: z.string(),
});

export type CreateChannelRequest = z.infer<typeof CreateChannelRequestSchema>;

export const UpdateChannelRequestSchema = z.object({
  name: z.string().min(1),
  settings: z.string(),
});

export type UpdateChannelRequest = z.infer<typeof UpdateChannelRequestSchema>;

export const NotificationEventTypeInfoSchema = z.object({
  event_type: z.string(),
  label: z.string(),
  priority: z.enum(['Low', 'Normal', 'High', 'Critical']),
});

export type NotificationEventTypeInfo = z.infer<
  typeof NotificationEventTypeInfoSchema
>;

export const UpdateSubscriptionsRequestSchema = z.object({
  events: z.array(z.string()),
});

export type UpdateSubscriptionsRequest = z.infer<
  typeof UpdateSubscriptionsRequestSchema
>;

// Form-specific schemas for each channel type (used for runtime validation)
export const DiscordChannelFormSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  channel_type: z.literal('Discord'),
  settings: DiscordSettingsSchema,
});

export const EmailChannelFormSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  channel_type: z.literal('Email'),
  settings: EmailSettingsSchema,
});

export const WebhookChannelFormSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  channel_type: z.literal('Webhook'),
  settings: WebhookSettingsSchema,
});

// Base schema for react-hook-form compatibility - uses generic record for settings
// and validates the settings based on channel_type via superRefine
// Base schema using discriminated union for type-safe settings validation
export const ChannelFormSchema = z.discriminatedUnion('channel_type', [
  DiscordChannelFormSchema,
  EmailChannelFormSchema,
  WebhookChannelFormSchema,
]);

export type ChannelFormData = z.infer<typeof ChannelFormSchema>;
