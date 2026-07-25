import { useEffect } from 'react';
import { useForm, useWatch, Resolver } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import {
  ChannelType,
  NotificationChannel,
  ChannelFormSchema,
  ChannelFormData,
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
import { WebhookForm } from './forms/webhook-form';
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

// Use the discriminated union schema from API schemas
type FormData = ChannelFormData;

/**
 * Starting settings for each channel type, matching the defaults on the schemas in
 * `api/schemas/notifications.ts`.
 *
 * The form holds one `settings` object shared by every type, so switching type has to re-seed it;
 * otherwise the previous type's keys linger and the new type's own fields come up empty.
 */
const DEFAULT_SETTINGS: Record<string, Record<string, unknown>> = {
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
};

export function ChannelForm({ channel, open, onOpenChange }: ChannelFormProps) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const isEditing = !!channel;

  const form = useForm<FormData>({
    resolver: zodResolver(ChannelFormSchema) as Resolver<FormData>,
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
      let settings: any = channel.settings;
      try {
        if (typeof settings === 'string') {
          settings = JSON.parse(settings);
        }
      } catch (e) {
        console.error('Failed to parse settings', e);
      }

      // Map the API response to form structure based on channel type
      if (channel.channel_type === 'Discord') {
        form.reset({
          name: channel.name,
          channel_type: 'Discord',
          settings: {
            webhook_url: settings.webhook_url || '',
            username: settings.username,
            avatar_url: settings.avatar_url,
            min_priority: settings.min_priority ?? 5,
            locale: settings.locale ?? '',
            enabled: settings.enabled !== false,
          },
        });
      } else if (channel.channel_type === 'Email') {
        form.reset({
          name: channel.name,
          channel_type: 'Email',
          settings: {
            smtp_host: settings.smtp_host || '',
            smtp_port: settings.smtp_port || 587,
            username: settings.username || '',
            password: settings.password || '',
            from_address: settings.from_address || '',
            to_addresses: settings.to_addresses || [],
            use_tls: settings.use_tls ?? true,
            min_priority: settings.min_priority ?? 8,
            locale: settings.locale ?? '',
            enabled: settings.enabled !== false,
          },
        });
      } else if (channel.channel_type === 'Telegram') {
        form.reset({
          name: channel.name,
          channel_type: 'Telegram',
          settings: {
            bot_token: settings.bot_token || '',
            chat_id: settings.chat_id || '',
            parse_mode: settings.parse_mode || 'HTML',
            min_priority: settings.min_priority ?? 5,
            locale: settings.locale ?? '',
            enabled: settings.enabled !== false,
          },
        });
      } else if (channel.channel_type === 'Gotify') {
        form.reset({
          name: channel.name,
          channel_type: 'Gotify',
          settings: {
            server_url: settings.server_url || '',
            app_token: settings.app_token || '',
            min_priority: settings.min_priority ?? 5,
            locale: settings.locale ?? '',
            enabled: settings.enabled !== false,
            timeout_secs: settings.timeout_secs || 30,
          },
        });
      } else if (channel.channel_type === 'Webhook') {
        // Map auth to discriminated union format
        let auth: any = { type: 'None' };
        if (settings.auth) {
          if (settings.auth.type === 'Bearer') {
            auth = { type: 'Bearer', token: settings.auth.token || '' };
          } else if (settings.auth.type === 'Basic') {
            auth = {
              type: 'Basic',
              username: settings.auth.username || '',
              password: settings.auth.password || '',
            };
          } else if (settings.auth.type === 'Header') {
            auth = {
              type: 'Header',
              name: settings.auth.name || '',
              value: settings.auth.value || '',
            };
          }
        }

        let headers: [string, string][] = [];
        if (settings.headers) {
          if (Array.isArray(settings.headers)) {
            headers = settings.headers;
          } else if (typeof settings.headers === 'object') {
            headers = Object.entries(settings.headers);
          }
        }

        form.reset({
          name: channel.name,
          channel_type: 'Webhook',
          settings: {
            url: settings.url || '',
            method: settings.method || 'POST',
            auth,
            min_priority: settings.min_priority ?? 2,
            locale: settings.locale ?? '',
            enabled: settings.enabled !== false,
            timeout_secs: settings.timeout_secs || 30,
            headers,
          },
        });
      }
    } else if (!channel && open) {
      form.reset({
        name: '',
        channel_type: 'Webhook',
        settings: DEFAULT_SETTINGS.Webhook as never,
      });
    }
  }, [channel, open, form]);

  const createMutation = useMutation({
    mutationFn: (data: any) => createChannel({ data }),
    onSuccess: () => {
      toast.success(i18n._(msg`Channel created`));
      void queryClient.invalidateQueries({
        queryKey: ['notification-channels'],
      });
      onOpenChange(false);
    },
    onError: (err: any) =>
      toast.error(err.message || i18n._(msg`Failed to create channel`)),
  });

  const updateMutation = useMutation({
    mutationFn: (data: any) =>
      updateChannel({ data: { id: channel!.id, data } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Channel updated`));
      void queryClient.invalidateQueries({
        queryKey: ['notification-channels'],
      });
      onOpenChange(false);
    },
    onError: (err: any) =>
      toast.error(err.message || i18n._(msg`Failed to update channel`)),
  });

  const onSubmit = (data: FormData) => {
    let finalSettings: any = { ...data.settings };

    // Transform headers array to object for Webhook
    if (data.channel_type === 'Webhook') {
      const settings = data.settings;
      console.log(settings);
      // Transform headers array to object
      const headersMap: Record<string, string> = {};
      if (settings.headers && Array.isArray(settings.headers)) {
        settings.headers.forEach(([key, value]: any) => {
          if (key) {
            headersMap[key] = value;
          }
        });
      }

      // Handle auth: if type is 'None', send null to backend
      const auth = settings.auth?.type === 'None' ? null : settings.auth;

      finalSettings = {
        ...settings,
        headers: headersMap,
        auth,
      };
      console.log('final ', finalSettings);
    }

    const payload = removeEmpty({
      name: data.name,
      channel_type: data.channel_type as ChannelType,
      settings: finalSettings,
    });

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
            onSubmit={form.handleSubmit(onSubmit as any)}
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
                            {/* Disabled rather than selectable-then-rejected: the previous
                              version accepted the click and answered with a warning toast. */}
                            <SelectItem value="Discord" disabled>
                              Discord <ComingSoon />
                            </SelectItem>
                            <SelectItem value="Email" disabled>
                              Email <ComingSoon />
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
