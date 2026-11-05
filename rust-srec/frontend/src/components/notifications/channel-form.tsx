import { useEffect } from 'react';
import { useForm, useWatch, Resolver } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import {
  ChannelType,
  NotificationChannel,
  ChannelFormSchema,
  ChannelFormData,
  type WebhookAuth,
  type WebhookAuthType,
} from '@/api/schemas';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Button } from '@/components/ui/button';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createChannel, updateChannel } from '@/server/functions/notifications';
import { toast } from 'sonner';
import { Bell, Box, Loader2, SlidersHorizontal } from 'lucide-react';
import { DiscordForm } from './forms/discord-form';
import { EmailForm } from './forms/email-form';
import { GotifyForm } from './forms/gotify-form';
import { TelegramForm } from './forms/telegram-form';
import { AUTH_DEFAULTS, WebhookForm } from './forms/webhook-form';
import { removeEmpty } from '@/lib/format';
import {
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';
import { IconInput } from '@/components/ui/icon-input';

interface ChannelFormProps {
  channel?: NotificationChannel | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Starting settings for each channel type, matching the defaults on the schemas in
 * `api/schemas/notifications.ts`.
 *
 * The form holds one `settings` object shared by every type, so switching type has to re-seed it;
 * otherwise the previous type's keys linger and the new type's own fields come up empty. An edited
 * channel's stored settings are laid over these too, so a key it was saved without still starts
 * with its default.
 */
export const DEFAULT_SETTINGS: Record<ChannelType, Record<string, unknown>> = {
  Discord: {
    webhook_url: '',
    min_priority: 5,
    locale: '',
    enabled: true,
  },
  Webhook: {
    url: '',
    method: 'POST',
    auth: { type: 'None' },
    min_priority: 2,
    locale: '',
    enabled: true,
    timeout_secs: 30,
    headers: [],
  },
  Telegram: {
    bot_token: '',
    chat_id: '',
    parse_mode: 'HTML',
    min_priority: 5,
    locale: '',
    enabled: true,
  },
  Gotify: {
    server_url: '',
    app_token: '',
    min_priority: 5,
    locale: '',
    enabled: true,
    timeout_secs: 30,
  },
  Email: {
    smtp_host: '',
    smtp_port: 587,
    // Empty strings rather than omitted keys: `EmailChannelSettings` declares
    // `username`/`password` as plain `String`, so a missing key fails deserialization.
    // `service.rs` maps the empty string to `None` for an unauthenticated relay.
    username: '',
    password: '',
    from_address: '',
    to_addresses: [],
    use_tls: true,
    min_priority: 8,
    locale: '',
    enabled: true,
  },
};

/** What the channel endpoints receive; `settings` is sent as the object, not JSON text. */
interface ChannelPayload {
  name: string;
  channel_type: ChannelType;
  settings: Record<string, unknown>;
}

/** A stored webhook `auth`, with every field of its type present so the inputs start filled. */
function webhookFormAuth(auth: unknown): WebhookAuth {
  const type =
    auth !== null && typeof auth === 'object' && 'type' in auth
      ? auth.type
      : undefined;
  if (typeof type !== 'string' || !(type in AUTH_DEFAULTS)) {
    return AUTH_DEFAULTS.None;
  }
  return {
    ...AUTH_DEFAULTS[type as WebhookAuthType],
    ...(auth as object),
  } as WebhookAuth;
}

/** Stored webhook headers, a name -> value map, as the editable rows of the form. */
function webhookFormHeaders(headers: unknown): [string, string][] {
  if (Array.isArray(headers)) return headers;
  if (headers !== null && typeof headers === 'object') {
    return Object.entries(headers).map(([name, value]) => [
      name,
      String(value),
    ]);
  }
  return [];
}

export function ChannelForm({ channel, open, onOpenChange }: ChannelFormProps) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const isEditing = !!channel;

  const form = useForm<ChannelFormData>({
    resolver: zodResolver(ChannelFormSchema) as Resolver<ChannelFormData>,
    defaultValues: {
      name: '',
      channel_type: 'Webhook',
      settings: DEFAULT_SETTINGS.Webhook as never,
    },
  });

  const selectedType = useWatch({
    control: form.control,
    name: 'channel_type',
  });

  // Re-seed settings whenever the type changes while creating. Editing keeps the loaded values,
  // and the type select is disabled there anyway.
  useEffect(() => {
    if (isEditing || !selectedType) return;
    const defaults = DEFAULT_SETTINGS[selectedType];
    if (!defaults) return;
    // `reset` rather than `setValue`: replacing the whole `settings` subtree has to re-register
    // the new type's fields, which is what makes their controlled inputs read the new values.
    form.reset({
      name: form.getValues('name'),
      channel_type: selectedType,
      settings: defaults as never,
    });
  }, [selectedType, isEditing, form]);

  // Load channel data when editing
  useEffect(() => {
    if (channel && open) {
      const stored = channel.settings ?? {};
      const settings: Record<string, unknown> = {
        ...DEFAULT_SETTINGS[channel.channel_type],
        ...stored,
      };
      if (channel.channel_type === 'Webhook') {
        settings.auth = webhookFormAuth(stored.auth);
        settings.headers = webhookFormHeaders(stored.headers);
      }
      form.reset({
        name: channel.name,
        channel_type: channel.channel_type,
        settings: settings as never,
      });
    } else if (!channel && open) {
      form.reset({
        name: '',
        channel_type: 'Webhook',
        settings: DEFAULT_SETTINGS.Webhook as never,
      });
    }
  }, [channel, open, form]);

  const createMutation = useMutation({
    mutationFn: (data: ChannelPayload) => createChannel({ data }),
    onSuccess: () => {
      toast.success(i18n._(msg`Channel created`));
      void queryClient.invalidateQueries({
        queryKey: ['notification-channels'],
      });
      onOpenChange(false);
    },
    onError: (err) =>
      toast.error(err.message || i18n._(msg`Failed to create channel`)),
  });

  const updateMutation = useMutation({
    mutationFn: ({ name, settings }: ChannelPayload) =>
      updateChannel({ data: { id: channel!.id, data: { name, settings } } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Channel updated`));
      void queryClient.invalidateQueries({
        queryKey: ['notification-channels'],
      });
      onOpenChange(false);
    },
    onError: (err) =>
      toast.error(err.message || i18n._(msg`Failed to update channel`)),
  });

  const onSubmit = (data: ChannelFormData) => {
    let settings: Record<string, unknown> = data.settings;
    if (data.channel_type === 'Webhook') {
      const { headers, auth } = data.settings;
      settings = {
        ...data.settings,
        // The backend keeps headers as a name -> value map; rows without a name are dropped.
        headers: Object.fromEntries((headers ?? []).filter(([name]) => name)),
        // No authentication is sent as `null`, which `removeEmpty` then leaves out.
        auth: auth?.type === 'None' ? null : auth,
      };
    }

    const payload: ChannelPayload = {
      name: data.name,
      channel_type: data.channel_type,
      settings: removeEmpty(settings),
    };

    if (isEditing) {
      updateMutation.mutate(payload);
    } else {
      createMutation.mutate(payload);
    }
  };

  const isPending = createMutation.isPending || updateMutation.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-[600px]">
        <DialogHeader className="shrink-0 border-b px-6 py-5">
          <div className="flex items-center gap-3">
            <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-primary/10">
              <Bell className="h-5 w-5 text-primary" />
            </span>
            <div className="space-y-0.5 text-left">
              <DialogTitle className="text-base">
                {isEditing ? (
                  <Trans>Edit channel</Trans>
                ) : (
                  <Trans>New notification channel</Trans>
                )}
              </DialogTitle>
              <DialogDescription className="text-sm">
                <Trans>
                  Configure where and how you receive notifications.
                </Trans>
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <Form {...form}>
          <form
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex min-h-0 flex-1 flex-col"
          >
            <div className="min-h-0 flex-1 space-y-6 overflow-y-auto px-6 py-6">
              <section className="space-y-4">
                <ConfigSectionHeading icon={Bell}>
                  <Trans>Channel</Trans>
                </ConfigSectionHeading>
                <div className="grid items-start gap-4 md:grid-cols-2">
                  <FormField
                    control={form.control}
                    name="name"
                    render={({ field }) => (
                      <FormItem className="space-y-2">
                        <ConfigFieldLabel>
                          <Trans>Name</Trans>
                        </ConfigFieldLabel>
                        <FormControl>
                          <IconInput
                            icon={Box}
                            placeholder={i18n._(msg`My Channel`)}
                            className={CONFIG_INPUT}
                            {...field}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={form.control}
                    name="channel_type"
                    render={({ field }) => (
                      <FormItem className="space-y-2">
                        <ConfigFieldLabel>
                          <Trans>Type</Trans>
                        </ConfigFieldLabel>
                        <Select
                          onValueChange={field.onChange}
                          value={field.value}
                          disabled={isEditing}
                        >
                          <FormControl>
                            <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                              <SelectValue
                                placeholder={i18n._(msg`Select type`)}
                              />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent className={CONFIG_SELECT_CONTENT}>
                            <SelectItem value="Webhook">Webhook</SelectItem>
                            <SelectItem value="Telegram">Telegram</SelectItem>
                            <SelectItem value="Gotify">Gotify</SelectItem>
                            <SelectItem value="Email">Email</SelectItem>
                            {/* Not offered for new channels yet. An existing Discord channel,
                              such as one imported or created through the API, still opens here
                              with its type shown. */}
                            <SelectItem value="Discord" disabled>
                              Discord <ComingSoon />
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </section>

              <section className="space-y-4">
                <ConfigSectionHeading icon={SlidersHorizontal}>
                  <Trans>Configuration</Trans>
                </ConfigSectionHeading>
                {selectedType === 'Webhook' && <WebhookForm />}
                {selectedType === 'Discord' && <DiscordForm />}
                {selectedType === 'Email' && <EmailForm />}
                {selectedType === 'Gotify' && <GotifyForm />}
                {selectedType === 'Telegram' && <TelegramForm />}
              </section>
            </div>

            <DialogFooter className="shrink-0 gap-2 border-t px-6 py-4">
              <Button
                type="button"
                variant="outline"
                onClick={() => onOpenChange(false)}
              >
                <Trans>Cancel</Trans>
              </Button>
              <Button type="submit" disabled={isPending} className="min-w-32">
                {isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {isEditing ? (
                  <Trans>Save changes</Trans>
                ) : (
                  <Trans>Create channel</Trans>
                )}
              </Button>
            </DialogFooter>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  );
}

/** Marks a channel type that exists in the schema but has no working integration yet. */
function ComingSoon() {
  return (
    <span className="ml-1 text-xs text-muted-foreground">
      (<Trans>Coming soon</Trans>)
    </span>
  );
}
