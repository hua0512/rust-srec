import { z } from 'zod';

export const ChannelTypeSchema = z.enum(['Discord', 'Email', 'Webhook']);
export type ChannelType = z.infer<typeof ChannelTypeSchema>;

// Settings schemas
export const DiscordSettingsSchema = z.object({
    webhook_url: z.url(),
    username: z.string().optional(),
    avatar_url: z.url().optional(),
});

export const EmailSettingsSchema = z.object({
    smtp_host: z.string().min(1),
    smtp_port: z.number().int().positive(),
    username: z.string().min(1),
    password: z.string().min(1),
    from_address: z.email(),
    to_addresses: z.array(z.string().email()).min(1),
    use_tls: z.boolean().default(true),
});

export const WebhookAuthTypeSchema = z.enum(['None', 'Bearer', 'Basic', 'Header']);
export type WebhookAuthType = z.infer<typeof WebhookAuthTypeSchema>;

export const WebhookAuthSchema = z.discriminatedUnion('type', [
    z.object({ type: z.literal('None') }),
    z.object({ type: z.literal('Bearer'), token: z.string() }),
    z.object({ type: z.literal('Basic'), username: z.string(), password: z.string() }),
    z.object({ type: z.literal('Header'), name: z.string(), value: z.string() }),
]);

export const WebhookSettingsSchema = z.object({
    url: z.url(),
    headers: z.array(z.tuple([z.string(), z.string()])).optional(), // Backend uses Vec<(String, String)>
    method: z.string().default('POST'),
    auth: WebhookAuthSchema.optional(),
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

export type NotificationEventTypeInfo = z.infer<typeof NotificationEventTypeInfoSchema>;

export const UpdateSubscriptionsRequestSchema = z.object({
    events: z.array(z.string()),
});

export type UpdateSubscriptionsRequest = z.infer<typeof UpdateSubscriptionsRequestSchema>;

// Form-specific discriminated union schemas for better type safety
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

export const ChannelFormSchema = z.discriminatedUnion('channel_type', [
    DiscordChannelFormSchema,
    EmailChannelFormSchema,
    WebhookChannelFormSchema,
]);

export type ChannelFormData = z.infer<typeof ChannelFormSchema>;
