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
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createChannel, updateChannel } from '@/server/functions/notifications';
import { toast } from 'sonner';
import { Loader2, Box } from 'lucide-react';
import { DiscordForm } from './forms/discord-form';
import { EmailForm } from './forms/email-form';
import { WebhookForm } from './forms/webhook-form';
import { removeEmpty } from '@/lib/format';

interface ChannelFormProps {
  channel?: NotificationChannel | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

// Use the discriminated union schema from API schemas
type FormData = ChannelFormData;

export function ChannelForm({ channel, open, onOpenChange }: ChannelFormProps) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const isEditing = !!channel;

  const form = useForm<FormData>({
    resolver: zodResolver(ChannelFormSchema) as Resolver<FormData>,
    defaultValues: {
      name: '',
      channel_type: 'Webhook',
      settings: {
        url: '',
        method: 'POST',
        auth: { type: 'None' },
        min_priority: 'Low',
        enabled: true,
        timeout_secs: 30,
        headers: [],
      },
    },
  });

  const selectedType = useWatch({
    control: form.control,
    name: 'channel_type',
  });

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
            min_priority: settings.min_priority || 'Normal',
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
            min_priority: settings.min_priority || 'High',
            enabled: settings.enabled !== false,
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
            min_priority: settings.min_priority || 'Low',
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
        settings: {
          url: '',
          method: 'POST',
          auth: { type: 'None' },
          min_priority: 'Low',
          enabled: true,
          timeout_secs: 30,
          headers: [],
        },
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
      <DialogContent className="sm:max-w-[600px] max-h-[90vh] overflow-y-auto bg-background/95 backdrop-blur-xl border-border/50">
        <DialogHeader className="pb-4 border-b border-border/40">
          <DialogTitle className="text-xl font-semibold tracking-tight">
            {isEditing
              ? i18n._(msg`Edit Channel`)
              : i18n._(msg`Create Channel`)}
          </DialogTitle>
          <DialogDescription>
            <Trans>Configure where and how you receive notifications.</Trans>
          </DialogDescription>
        </DialogHeader>

        <Form {...form}>
          <form
            onSubmit={form.handleSubmit(onSubmit as any)}
            className="space-y-6 pt-4"
          >
            <div className="grid gap-6">
              {/* General Settings */}
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <FormField
                  control={form.control}
                  name="name"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-muted-foreground/80">
                        <Trans>Name</Trans>
                      </FormLabel>
                      <FormControl>
                        <div className="relative">
                          <Box className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                          <Input
                            placeholder={i18n._(msg`My Channel`)}
                            {...field}
                            className="pl-9 bg-muted/30 border-primary/10"
                          />
                        </div>
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="channel_type"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-muted-foreground/80">
                        <Trans>Type</Trans>
                      </FormLabel>
                      <Select
                        onValueChange={(val) => {
                          if (val === 'Discord' || val === 'Email') {
                            toast.warning(
                              i18n._(
                                msg`${val} channel support is coming soon!`,
                              ),
                            );
                            return;
                          }
                          field.onChange(val);
                        }}
                        defaultValue={field.value}
                        disabled={isEditing}
                      >
                        <FormControl>
                          <SelectTrigger className="bg-muted/30 border-primary/10">
                            <SelectValue
                              placeholder={i18n._(msg`Select type`)}
                            />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          <SelectItem value="Webhook">Webhook</SelectItem>
                          <SelectItem value="Discord">
                            Discord (Coming Soon)
                          </SelectItem>
                          <SelectItem value="Email">
                            Email (Coming Soon)
                          </SelectItem>
                        </SelectContent>
                      </Select>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              {/* Separator */}
              <div className="relative">
                <div className="absolute inset-0 flex items-center">
                  <span className="w-full border-t border-border/40" />
                </div>
                <div className="relative flex justify-center text-xs uppercase">
                  <span className="bg-background/95 px-2 text-muted-foreground">
                    <Trans>Configuration</Trans>
                  </span>
                </div>
              </div>

              {/* Dynamic Forms */}
              {selectedType === 'Webhook' && <WebhookForm />}
              {selectedType === 'Discord' && <DiscordForm />}
              {selectedType === 'Email' && <EmailForm />}
            </div>

            <DialogFooter className="pt-4 border-t border-border/40">
              <Button
                type="button"
                variant="outline"
                onClick={() => onOpenChange(false)}
              >
                <Trans>Cancel</Trans>
              </Button>
              <Button
                type="submit"
                disabled={isPending}
                className="bg-primary text-primary-foreground shadow-lg shadow-primary/20"
              >
                {isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {isEditing
                  ? i18n._(msg`Update Channel`)
                  : i18n._(msg`Create Channel`)}
              </Button>
            </DialogFooter>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  );
}
