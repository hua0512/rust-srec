import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Card, CardContent } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { useEffect, useState } from 'react';

interface EventHooks {
  on_online?: string;
  on_offline?: string;
  on_download_start?: string;
  on_download_complete?: string;
  on_download_error?: string;
  on_pipeline_complete?: string;
}

interface EventHooksFormProps {
  form: UseFormReturn<any>;
  name: string;
  mode?: 'json' | 'object';
}

export function EventHooksForm({
  form,
  name,
  mode = 'object',
}: EventHooksFormProps) {
  const currentVal = form.watch(name);
  const [hooks, setHooks] = useState<EventHooks>({});

  useEffect(() => {
    if (currentVal) {
      if (mode === 'json' && typeof currentVal === 'string') {
        try {
          setHooks(JSON.parse(currentVal));
        } catch (e) {
          console.warn('Invalid EventHooks JSON', e);
        }
      } else if (typeof currentVal === 'object') {
        setHooks(currentVal);
      }
    } else {
      setHooks({});
    }
  }, [currentVal, mode]);

  const updateField = (key: keyof EventHooks, value: string) => {
    const newHooks = { ...hooks, [key]: value || undefined };
    // Remove undefined keys
    Object.keys(newHooks).forEach((k) => {
      const castKey = k as keyof EventHooks;
      if (newHooks[castKey] === undefined) delete newHooks[castKey];
    });

    setHooks(newHooks);

    // For object mode, we can't easily partially update if the parent form expects an object and we are just a sub-component.
    // Actually, creating a new object and setting it is fine.
    // If mode is object, we write object. If mode is string, we write string.

    if (mode === 'json') {
      form.setValue(
        name,
        Object.keys(newHooks).length > 0 ? JSON.stringify(newHooks) : null,
        {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        },
      );
    } else {
      // For object mode, we might need to be careful if 'name' points to 'streamer_specific_config.event_hooks' which is an object.
      // We can just set the value.
      form.setValue(name, newHooks, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
    }
  };

  const renderField = (
    key: keyof EventHooks,
    label: React.ReactNode,
    placeholder: string,
    description: React.ReactNode,
  ) => (
    <FormItem>
      <FormLabel>{label}</FormLabel>
      <FormControl>
        <Input
          placeholder={placeholder}
          value={hooks[key] ?? ''}
          onChange={(e) => updateField(key, e.target.value)}
          className="font-mono text-sm"
        />
      </FormControl>
      <FormDescription>{description}</FormDescription>
    </FormItem>
  );

  return (
    <Card className="border-dashed shadow-none">
      <CardContent className="pt-6 space-y-4">
        <p className="text-sm text-muted-foreground mb-4">
          <Trans>
            Execute shell commands on lifecycle events. Commands run in the
            system shell.
          </Trans>
        </p>

        {renderField(
          'on_online',
          <Trans>On Online</Trans>,
          "notify-send 'Streamer is online!'",
          <Trans>Runs when the streamer goes online.</Trans>,
        )}
        {renderField(
          'on_offline',
          <Trans>On Offline</Trans>,
          "echo 'Streamer went offline'",
          <Trans>Runs when the streamer goes offline.</Trans>,
        )}
        {renderField(
          'on_download_start',
          <Trans>On Download Start</Trans>,
          "echo 'Download started'",
          <Trans>Runs when a download begins.</Trans>,
        )}
        {renderField(
          'on_download_complete',
          <Trans>On Download Complete</Trans>,
          "echo 'Download finished: $FILE_PATH'",
          <Trans>Runs when a download completes successfully.</Trans>,
        )}
        {renderField(
          'on_download_error',
          <Trans>On Download Error</Trans>,
          "echo 'Download failed: $ERROR'",
          <Trans>Runs when a download encounters an error.</Trans>,
        )}
        {renderField(
          'on_pipeline_complete',
          <Trans>On Pipeline Complete</Trans>,
          "echo 'Pipeline finished'",
          <Trans>Runs when the post-processing pipeline completes.</Trans>,
        )}
      </CardContent>
    </Card>
  );
}
