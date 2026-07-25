import React from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { Button } from '@/components/ui/button';
import {
  Bot,
  CalendarClock,
  Cpu,
  Database,
  Globe,
  KeyRound,
  ListMusic,
  Share2,
} from 'lucide-react';
import { useDefaultPlaceholder } from '@/hooks/use-default-placeholder';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';
import { cn } from '@/lib/utils';
import { Card, CardContent } from '@/components/ui/card';

interface SubFormProps {
  hlsPath: string;
}

type TriStateMode = 'default' | 'disabled' | 'custom';

type GapSkipStrategyType =
  | 'wait_indefinitely'
  | 'skip_after_count'
  | 'skip_after_duration'
  | 'skip_after_both';

type VariantSelectionType =
  | 'highest_bitrate'
  | 'lowest_bitrate'
  | 'closest_to_bitrate'
  | 'audio_only'
  | 'video_only'
  | 'matching_resolution'
  | 'custom';

function KeyValuePairsEditor({
  label,
  description,
  path,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  path: string;
}) {
  const { setValue, control } = useFormContext<any>();
  const value =
    (useWatch({ control, name: path }) as
      | Array<[string, string]>
      | undefined) ?? [];

  const addEntry = () => {
    setValue(path, [...value, ['', '']], { shouldDirty: true });
  };

  const updateEntry = (idx: number, next: [string, string]) => {
    const nextValue = value.map((pair, i) => (i === idx ? next : pair));
    setValue(path, nextValue, { shouldDirty: true });
  };

  const removeEntry = (idx: number) => {
    const nextValue = value.filter((_, i) => i !== idx);
    setValue(path, nextValue.length > 0 ? nextValue : undefined, {
      shouldDirty: true,
    });
  };

  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <div className="space-y-0.5">
          <div className="text-xs font-medium">{label}</div>
          {description && (
            <div className="text-[10px] text-muted-foreground">
              {description}
            </div>
          )}
        </div>

        <div className="space-y-2">
          {value.length === 0 && (
            <div className="text-[10px] text-muted-foreground">
              <Trans>No parameters configured.</Trans>
            </div>
          )}

          {value.map(([k, v], idx) => (
            <div key={idx} className="grid grid-cols-1 sm:grid-cols-5 gap-2">
              <Input
                value={k}
                onChange={(e) => updateEntry(idx, [e.target.value, v])}
                className={cn(CONFIG_INPUT, 'font-mono sm:col-span-2')}
                placeholder="key"
              />
              <Input
                value={v}
                onChange={(e) => updateEntry(idx, [k, e.target.value])}
                className={cn(CONFIG_INPUT, 'font-mono sm:col-span-2')}
                placeholder="value"
              />
              <Button
                type="button"
                variant="outline"
                className={CONFIG_INPUT}
                onClick={() => removeEntry(idx)}
              >
                <Trans>Remove</Trans>
              </Button>
            </div>
          ))}

          <Button
            type="button"
            variant="outline"
            className={cn(CONFIG_INPUT, 'w-full')}
            onClick={addEntry}
          >
            <Trans>Add parameter</Trans>
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function TriStateNullableDurationMsField({
  label,
  description,
  path,
  placeholder,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  path: string;
  placeholder: string;
}) {
  const { setValue, control } = useFormContext<any>();
  const raw = useWatch({ control, name: path }) as unknown;
  // react-hook-form can hold `''` (empty string) from the input.
  // Treat that as "unset" so the mode reflects "Default".
  const normalizedRaw = raw === '' ? undefined : raw;

  const mode: TriStateMode =
    normalizedRaw === null
      ? 'disabled'
      : normalizedRaw === undefined
        ? 'default'
        : 'custom';

  const setMode = (next: TriStateMode) => {
    if (next === 'default') {
      setValue(path, undefined, { shouldDirty: true });
      return;
    }
    if (next === 'disabled') {
      setValue(path, null, { shouldDirty: true });
      return;
    }

    // custom
    if (normalizedRaw === undefined || normalizedRaw === null) {
      // Seed with a reasonable value so "Custom" is never ambiguous.
      const seeded = Number.isFinite(Number(placeholder))
        ? Number(placeholder)
        : 10000;
      setValue(path, seeded, { shouldDirty: true });
    }
  };

  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-2">
        <div className="space-y-0.5">
          <div className="text-xs font-medium">{label}</div>
          {description && (
            <div className="text-[10px] text-muted-foreground">
              {description}
            </div>
          )}
        </div>

        <Select value={mode} onValueChange={(v) => setMode(v as TriStateMode)}>
          <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={CONFIG_SELECT_CONTENT}>
            <SelectItem value="default">
              <Trans>Default</Trans>
            </SelectItem>
            <SelectItem value="disabled">
              <Trans>Disabled</Trans>
            </SelectItem>
            <SelectItem value="custom">
              <Trans>Custom</Trans>
            </SelectItem>
          </SelectContent>
        </Select>

        {mode === 'custom' && (
          <Input
            type="number"
            value={
              typeof normalizedRaw === 'number' ||
              typeof normalizedRaw === 'string'
                ? normalizedRaw
                : ''
            }
            onChange={(e) =>
              setValue(
                path,
                e.target.value === '' ? undefined : e.target.value,
                {
                  shouldDirty: true,
                },
              )
            }
            className={cn(CONFIG_INPUT, 'font-mono')}
            placeholder={placeholder}
          />
        )}
      </CardContent>
    </Card>
  );
}

function GapSkipStrategyField({
  label,
  path,
}: {
  label: React.ReactNode;
  path: string;
}) {
  const { setValue, control } = useFormContext<any>();
  const value = useWatch({ control, name: path }) as
    | {
        type?: GapSkipStrategyType;
        count?: number | string;
        duration_ms?: number | string;
      }
    | undefined;

  const type: 'default' | GapSkipStrategyType =
    value?.type != null ? (value.type as GapSkipStrategyType) : 'default';

  const setType = (t: 'default' | GapSkipStrategyType) => {
    if (t === 'default') {
      setValue(path, undefined, { shouldDirty: true });
      return;
    }
    if (t === 'wait_indefinitely') {
      setValue(path, { type: 'wait_indefinitely' }, { shouldDirty: true });
      return;
    }
    if (t === 'skip_after_count') {
      setValue(
        path,
        { type: 'skip_after_count', count: 10 },
        { shouldDirty: true },
      );
      return;
    }
    if (t === 'skip_after_duration') {
      setValue(
        path,
        { type: 'skip_after_duration', duration_ms: 5000 },
        { shouldDirty: true },
      );
      return;
    }
    setValue(
      path,
      { type: 'skip_after_both', count: 10, duration_ms: 5000 },
      { shouldDirty: true },
    );
  };

  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <div className="text-xs font-medium">{label}</div>

        <Select value={type} onValueChange={(v) => setType(v as typeof type)}>
          <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={CONFIG_SELECT_CONTENT}>
            <SelectItem value="default">
              <Trans>Default</Trans>
            </SelectItem>
            <SelectItem value="wait_indefinitely">
              <Trans>Wait indefinitely</Trans>
            </SelectItem>
            <SelectItem value="skip_after_count">
              <Trans>Skip after count</Trans>
            </SelectItem>
            <SelectItem value="skip_after_duration">
              <Trans>Skip after duration</Trans>
            </SelectItem>
            <SelectItem value="skip_after_both">
              <Trans>Skip after both</Trans>
            </SelectItem>
          </SelectContent>
        </Select>

        {type === 'skip_after_count' && (
          <FormField
            name={`${path}.count`}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel size="sm">
                  <Trans>Count</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className={cn(CONFIG_INPUT, 'font-mono')}
                    placeholder="10"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        )}

        {type === 'skip_after_duration' && (
          <FormField
            name={`${path}.duration_ms`}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel size="sm">
                  <Trans>Duration (ms)</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className={cn(CONFIG_INPUT, 'font-mono')}
                    placeholder="5000"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        )}

        {type === 'skip_after_both' && (
          <div className="grid gap-3 sm:grid-cols-2">
            <FormField
              name={`${path}.count`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel size="sm">
                    <Trans>Count</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className={cn(CONFIG_INPUT, 'font-mono')}
                      placeholder="10"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              name={`${path}.duration_ms`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel size="sm">
                    <Trans>Duration (ms)</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className={cn(CONFIG_INPUT, 'font-mono')}
                      placeholder="5000"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function VariantSelectionPolicyField({
  label,
  path,
}: {
  label: React.ReactNode;
  path: string;
}) {
  const { setValue, control } = useFormContext<any>();
  const value = useWatch({ control, name: path }) as
    | {
        type?: VariantSelectionType;
        target_bitrate?: number | string;
        width?: number | string;
        height?: number | string;
        value?: string;
      }
    | undefined;

  const type: 'default' | VariantSelectionType =
    value?.type != null ? (value.type as VariantSelectionType) : 'default';

  const setType = (t: 'default' | VariantSelectionType) => {
    if (t === 'default') {
      setValue(path, undefined, { shouldDirty: true });
      return;
    }

    if (t === 'highest_bitrate') {
      setValue(path, { type: 'highest_bitrate' }, { shouldDirty: true });
      return;
    }
    if (t === 'lowest_bitrate') {
      setValue(path, { type: 'lowest_bitrate' }, { shouldDirty: true });
      return;
    }
    if (t === 'audio_only') {
      setValue(path, { type: 'audio_only' }, { shouldDirty: true });
      return;
    }
    if (t === 'video_only') {
      setValue(path, { type: 'video_only' }, { shouldDirty: true });
      return;
    }
    if (t === 'closest_to_bitrate') {
      setValue(
        path,
        { type: 'closest_to_bitrate', target_bitrate: 0 },
        { shouldDirty: true },
      );
      return;
    }
    if (t === 'matching_resolution') {
      setValue(
        path,
        { type: 'matching_resolution', width: 1920, height: 1080 },
        { shouldDirty: true },
      );
      return;
    }

    setValue(path, { type: 'custom', value: '' }, { shouldDirty: true });
  };

  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <div className="text-xs font-medium">{label}</div>
        <Select value={type} onValueChange={(v) => setType(v as typeof type)}>
          <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={CONFIG_SELECT_CONTENT}>
            <SelectItem value="default">
              <Trans>Default</Trans>
            </SelectItem>
            <SelectItem value="highest_bitrate">
              <Trans>Highest bitrate</Trans>
            </SelectItem>
            <SelectItem value="lowest_bitrate">
              <Trans>Lowest bitrate</Trans>
            </SelectItem>
            <SelectItem value="closest_to_bitrate">
              <Trans>Closest to bitrate</Trans>
            </SelectItem>
            <SelectItem value="audio_only">
              <Trans>Audio only</Trans>
            </SelectItem>
            <SelectItem value="video_only">
              <Trans>Video only</Trans>
            </SelectItem>
            <SelectItem value="matching_resolution">
              <Trans>Matching resolution</Trans>
            </SelectItem>
            <SelectItem value="custom">
              <Trans>Custom</Trans>
            </SelectItem>
          </SelectContent>
        </Select>

        {type === 'closest_to_bitrate' && (
          <FormField
            name={`${path}.target_bitrate`}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel size="sm">
                  <Trans>Target bitrate</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className={cn(CONFIG_INPUT, 'font-mono')}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        )}

        {type === 'matching_resolution' && (
          <div className="grid gap-3 sm:grid-cols-2">
            <FormField
              name={`${path}.width`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel size="sm">
                    <Trans>Width</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className={cn(CONFIG_INPUT, 'font-mono')}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              name={`${path}.height`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel size="sm">
                    <Trans>Height</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className={cn(CONFIG_INPUT, 'font-mono')}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        )}

        {type === 'custom' && (
          <FormField
            name={`${path}.value`}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel size="sm">
                  <Trans>Value</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input {...field} className={cn(CONFIG_INPUT, 'font-mono')} />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        )}
      </CardContent>
    </Card>
  );
}

function DecryptionOffloadToggle({
  label,
  description,
  path,
  defaultChecked,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  path: string;
  defaultChecked: boolean;
}) {
  const { setValue, control } = useFormContext<any>();
  const value = useWatch({ control, name: path }) as boolean | undefined;

  const checked = value ?? defaultChecked;

  return (
    <div className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
      <div className="space-y-0.5">
        <div className="text-[11px] font-normal">{label}</div>
        {description && (
          <div className="text-[10px] text-muted-foreground">{description}</div>
        )}
      </div>
      <Switch
        checked={checked}
        onCheckedChange={(next) =>
          setValue(path, next, {
            shouldDirty: true,
          })
        }
        className="scale-75 origin-right"
      />
    </div>
  );
}

const HlsBaseSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const { i18n } = useLingui();
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.base.timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Global Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder('0 (No timeout)')}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.connect_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Connect Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(30000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.read_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Read Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(30000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.write_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Write Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(30000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <KeyValuePairsEditor
        label={<Trans>Query Parameters</Trans>}
        description={
          <Trans>
            Appended to all HLS requests. Useful for signed URLs or CDN routing.
          </Trans>
        }
        path={`${hlsPath}.base.params`}
      />

      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.base.user_agent`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>User Agent</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder('Mozilla/5.0...')}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.http_version`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>HTTP Version Preference</Trans>
              </ConfigFieldLabel>
              <Select
                onValueChange={field.onChange}
                defaultValue={field.value || 'auto'}
              >
                <FormControl>
                  <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                    <SelectValue placeholder={i18n._(msg`Auto`)} />
                  </SelectTrigger>
                </FormControl>
                <SelectContent className={CONFIG_SELECT_CONTENT}>
                  <SelectItem value="auto">
                    <Trans>Auto (Default)</Trans>
                  </SelectItem>
                  <SelectItem value="http2_only">
                    <Trans>HTTP/2 Only</Trans>
                  </SelectItem>
                  <SelectItem value="http1_only">
                    <Trans>HTTP/1.1 Only</Trans>
                  </SelectItem>
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <div className="grid gap-4 sm:grid-cols-3">
        <FormField
          name={`${hlsPath}.base.http2_keep_alive_interval_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>H2 Keep-Alive (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(20000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.pool_max_idle_per_host`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Idle per Host</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(10)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.pool_idle_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Pool Idle Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(30000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <div className="grid gap-2 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.base.follow_redirects`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
              <ConfigFieldLabel size="sm">
                <Trans>Follow Redirects (Default: On)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? true}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.danger_accept_invalid_certs`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
              <ConfigFieldLabel size="sm">
                <Trans>Accept Invalid Certs (Default: Off)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? false}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.force_ipv4`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
              <ConfigFieldLabel size="sm">
                <Trans>Force IPv4 (Default: Off)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? false}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.base.force_ipv6`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
              <ConfigFieldLabel size="sm">
                <Trans>Force IPv6 (Default: Off)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? false}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
      </div>
    </div>
  );
});
HlsBaseSettings.displayName = 'HlsBaseSettings';

const HlsPlaylistSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.playlist_config.initial_playlist_fetch_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Initial Fetch Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(15000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.playlist_config.live_refresh_interval_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Live Refresh Interval (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(1000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.playlist_config.live_max_refresh_retries`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Refresh Retries</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(5)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.playlist_config.live_refresh_retry_delay_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Retry Delay (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(1000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <Card className="border-border/40 bg-muted/5">
        <CardContent className="p-3 space-y-3">
          <VariantSelectionPolicyField
            label={<Trans>Variant Selection Policy</Trans>}
            path={`${hlsPath}.playlist_config.variant_selection_policy`}
          />

          <FormField
            name={`${hlsPath}.playlist_config.adaptive_refresh_enabled`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between">
                <div className="space-y-0.5">
                  <ConfigFieldLabel>
                    <Trans>Adaptive Refresh (Default: On)</Trans>
                  </ConfigFieldLabel>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Adjust rate based on target duration</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value ?? true}
                    onCheckedChange={field.onChange}
                    className="scale-75 origin-right"
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <div className="grid gap-3 sm:grid-cols-2 pt-2 border-t border-border/40">
            <FormField
              name={`${hlsPath}.playlist_config.adaptive_refresh_min_interval_ms`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel size="sm">
                    <Trans>Min Interval (ms)</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className="h-7 text-xs font-mono"
                      placeholder={defaultPlaceholder(500)}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              name={`${hlsPath}.playlist_config.adaptive_refresh_max_interval_ms`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel size="sm">
                    <Trans>Max Interval (ms)</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className="h-7 text-xs font-mono"
                      placeholder={defaultPlaceholder(3000)}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </CardContent>
      </Card>
    </div>
  );
});
HlsPlaylistSettings.displayName = 'HlsPlaylistSettings';

const HlsSchedulerSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.scheduler_config.download_concurrency`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Download Concurrency</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(5)}
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Maximum number of concurrent segment downloads.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          name={`${hlsPath}.scheduler_config.processed_segment_buffer_multiplier`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Processed Buffer Multiplier</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(4)}
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>
                  Channel buffer size multiplier for processed segments.
                </Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  );
});
HlsSchedulerSettings.displayName = 'HlsSchedulerSettings';

const HlsFetcherSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.fetcher_config.segment_download_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Segment Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(10000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.max_segment_retries`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Segment Retries</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(3)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.segment_retry_delay_base_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Retry Delay Base (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(500)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.max_segment_retry_delay_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Retry Delay (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(10000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.key_download_timeout_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel size="sm">
                <Trans>Key Timeout (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(5000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.max_key_retries`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel size="sm">
                <Trans>Max Key Retries</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(3)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.key_retry_delay_base_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel size="sm">
                <Trans>Key Retry Delay (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(200)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.fetcher_config.max_key_retry_delay_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel size="sm">
                <Trans>Max Key Retry Delay (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(5000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  );
});
HlsFetcherSettings.displayName = 'HlsFetcherSettings';

const HlsProcessorSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <FormField
        name={`${hlsPath}.processor_config.processed_segment_ttl_ms`}
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Processed Segment TTL (ms)</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className={cn(CONFIG_INPUT, 'font-mono')}
                placeholder={defaultPlaceholder(60000)}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>
                How long to keep decrypted/processed segments in cache.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  );
});
HlsProcessorSettings.displayName = 'HlsProcessorSettings';

const HlsDecryptionSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.decryption_config.key_cache_ttl_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Key Cache TTL (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(3600000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <DecryptionOffloadToggle
          label={<Trans>Offload Decryption (Default: On)</Trans>}
          description={
            <Trans>
              Runs decryption on a blocking thread pool to avoid stalling async
              tasks.
            </Trans>
          }
          path={`${hlsPath}.decryption_config.offload_decryption_to_cpu_pool`}
          defaultChecked={true}
        />
      </div>
    </div>
  );
});
HlsDecryptionSettings.displayName = 'HlsDecryptionSettings';

const HlsCacheSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-3">
        <FormField
          name={`${hlsPath}.cache_config.playlist_ttl_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Playlist TTL (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(60000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.cache_config.segment_ttl_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Segment TTL (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(120000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.cache_config.decryption_key_ttl_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Decryption Key TTL (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(3600000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  );
});
HlsCacheSettings.displayName = 'HlsCacheSettings';

const HlsOutputSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          name={`${hlsPath}.output_config.live_reorder_buffer_duration_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Reorder Duration (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(30000)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.output_config.live_reorder_buffer_max_segments`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Reorder Max Segments</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(10)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${hlsPath}.output_config.gap_evaluation_interval_ms`}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Gap Eval Interval (ms)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                  placeholder={defaultPlaceholder(200)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <TriStateNullableDurationMsField
          label={<Trans>Max Stall Duration (ms)</Trans>}
          description={
            <Trans>
              Default uses Mesio’s built-in live stall timeout. Disabled means
              wait indefinitely.
            </Trans>
          }
          path={`${hlsPath}.output_config.live_max_overall_stall_duration_ms`}
          placeholder="60000"
        />
      </div>

      <FormField
        name={`${hlsPath}.output_config.max_pending_init_segments`}
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Max Pending Init Segments</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className={cn(CONFIG_INPUT, 'font-mono')}
                placeholder={defaultPlaceholder(8)}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>0 disables the limit.</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <div className="grid gap-4 sm:grid-cols-2">
        <GapSkipStrategyField
          label={<Trans>Live Gap Strategy</Trans>}
          path={`${hlsPath}.output_config.live_gap_strategy`}
        />
        <GapSkipStrategyField
          label={<Trans>VOD Gap Strategy</Trans>}
          path={`${hlsPath}.output_config.vod_gap_strategy`}
        />
      </div>

      <TriStateNullableDurationMsField
        label={<Trans>VOD Segment Timeout (ms)</Trans>}
        description={
          <Trans>
            When enabled, each VOD segment must complete within this timeout.
          </Trans>
        }
        path={`${hlsPath}.output_config.vod_segment_timeout_ms`}
        placeholder="30000"
      />

      <FormField
        name={`${hlsPath}.output_config.metrics_enabled`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <ConfigFieldLabel size="sm">
              <Trans>Enable Output Metrics (Default: On)</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Switch
                checked={field.value ?? true}
                onCheckedChange={field.onChange}
                className="scale-75 origin-right"
              />
            </FormControl>
          </FormItem>
        )}
      />

      <div className="space-y-3">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground border-b border-border/40 pb-1">
          <Trans>Buffer Limits</Trans>
        </h4>
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField
            name={`${hlsPath}.output_config.buffer_limits.max_segments`}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel size="sm">
                  <Trans>Max Segments</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className={cn(CONFIG_INPUT, 'font-mono')}
                    placeholder={defaultPlaceholder(50)}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            name={`${hlsPath}.output_config.buffer_limits.max_bytes`}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel size="sm">
                  <Trans>Max Bytes</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className={cn(CONFIG_INPUT, 'font-mono')}
                    placeholder={defaultPlaceholder('104857600 (100 MiB)')}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
      </div>
    </div>
  );
});
HlsOutputSettings.displayName = 'HlsOutputSettings';

interface MesioHlsFormProps {
  basePath?: string;
}

/**
 * The eight HLS setting groups.
 *
 * Driven from data rather than eight hand-written triggers, which had drifted to the point that
 * three of them shared the same icon while the labels were hidden below `sm` — leaving a row of
 * identical glyphs as the only navigation on small screens.
 */
const HLS_SECTIONS = [
  { value: 'base', icon: Globe, label: msg`Base`, Section: HlsBaseSettings },
  {
    value: 'playlist',
    icon: ListMusic,
    label: msg`Playlist`,
    Section: HlsPlaylistSettings,
  },
  {
    value: 'scheduler',
    icon: CalendarClock,
    label: msg`Scheduler`,
    Section: HlsSchedulerSettings,
  },
  {
    value: 'fetcher',
    icon: Bot,
    label: msg`Fetcher`,
    Section: HlsFetcherSettings,
  },
  {
    value: 'processor',
    icon: Cpu,
    label: msg`Processor`,
    Section: HlsProcessorSettings,
  },
  {
    value: 'decryption',
    icon: KeyRound,
    label: msg`Decryption`,
    Section: HlsDecryptionSettings,
  },
  {
    value: 'cache',
    icon: Database,
    label: msg`Cache`,
    Section: HlsCacheSettings,
  },
  {
    value: 'output',
    icon: Share2,
    label: msg`Output`,
    Section: HlsOutputSettings,
  },
];

export function MesioHlsForm({ basePath = 'config' }: MesioHlsFormProps) {
  const { i18n } = useLingui();
  const hlsPath = `${basePath}.hls`;

  return (
    <Accordion type="multiple" defaultValue={['base']} className="w-full">
      {HLS_SECTIONS.map(({ value, icon: Icon, label, Section }) => (
        <AccordionItem key={value} value={value} className="border-border/50">
          <AccordionTrigger className="py-3 hover:no-underline">
            <span className="flex items-center gap-2.5">
              <Icon className="h-4 w-4 text-muted-foreground" />
              <span className="text-sm font-medium">{i18n._(label)}</span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="pb-4 pt-1">
            <Section hlsPath={hlsPath} />
          </AccordionContent>
        </AccordionItem>
      ))}
    </Accordion>
  );
}
