import React from 'react';
import { Control, useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Trans } from '@lingui/react/macro';
import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Globe, ListMusic, Bot, Zap, Share2 } from 'lucide-react';

interface SubFormProps {
  control: Control<any>;
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
                className="h-8 text-xs font-mono sm:col-span-2"
                placeholder="key"
              />
              <Input
                value={v}
                onChange={(e) => updateEntry(idx, [k, e.target.value])}
                className="h-8 text-xs font-mono sm:col-span-2"
                placeholder="value"
              />
              <Button
                type="button"
                variant="outline"
                className="h-8 text-xs"
                onClick={() => removeEntry(idx)}
              >
                <Trans>Remove</Trans>
              </Button>
            </div>
          ))}

          <Button
            type="button"
            variant="outline"
            className="h-8 text-xs w-full"
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
          <SelectTrigger className="h-8 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
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
            className="h-8 text-xs font-mono"
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
          <SelectTrigger className="h-8 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
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
            control={control}
            name={`${path}.count`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Count</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
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
            control={control}
            name={`${path}.duration_ms`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Duration (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
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
              control={control}
              name={`${path}.count`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-[10px] text-muted-foreground">
                    <Trans>Count</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className="h-8 text-xs font-mono"
                      placeholder="10"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${path}.duration_ms`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-[10px] text-muted-foreground">
                    <Trans>Duration (ms)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className="h-8 text-xs font-mono"
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
          <SelectTrigger className="h-8 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
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
            control={control}
            name={`${path}.target_bitrate`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Target bitrate</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
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
              control={control}
              name={`${path}.width`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-[10px] text-muted-foreground">
                    <Trans>Width</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className="h-8 text-xs font-mono"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${path}.height`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-[10px] text-muted-foreground">
                    <Trans>Height</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      className="h-8 text-xs font-mono"
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
            control={control}
            name={`${path}.value`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Value</Trans>
                </FormLabel>
                <FormControl>
                  <Input {...field} className="h-8 text-xs font-mono" />
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

const HlsBaseSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.base.timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Global Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 0 (No timeout)"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.connect_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Connect Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.read_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Read Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.write_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Write Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
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
        control={control}
        name={`${hlsPath}.base.user_agent`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>User Agent</Trans>
            </FormLabel>
            <FormControl>
              <Input
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: Mozilla/5.0..."
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.http_version`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>HTTP Version Preference</Trans>
            </FormLabel>
            <Select
              onValueChange={field.onChange}
              defaultValue={field.value || 'auto'}
            >
              <FormControl>
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue placeholder="Auto" />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
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
        control={control}
        name={`${hlsPath}.base.http2_keep_alive_interval_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>H2 Keep-Alive (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 20000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.pool_max_idle_per_host`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Idle per Host</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.pool_idle_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Pool Idle Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="grid gap-2 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.base.follow_redirects`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal">
              <Trans>Follow Redirects (Default: On)</Trans>
            </FormLabel>
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
        control={control}
        name={`${hlsPath}.base.danger_accept_invalid_certs`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal text-destructive/80">
              <Trans>Accept Invalid Certs (Default: Off)</Trans>
            </FormLabel>
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
        control={control}
        name={`${hlsPath}.base.force_ipv4`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal">
              <Trans>Force IPv4 (Default: Off)</Trans>
            </FormLabel>
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
        control={control}
        name={`${hlsPath}.base.force_ipv6`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal">
              <Trans>Force IPv6 (Default: Off)</Trans>
            </FormLabel>
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
));
HlsBaseSettings.displayName = 'HlsBaseSettings';

const HlsPlaylistSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.initial_playlist_fetch_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Initial Fetch Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 15000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.live_refresh_interval_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Live Refresh Interval (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 1000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.live_max_refresh_retries`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Refresh Retries</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 5"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.live_refresh_retry_delay_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Retry Delay (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 1000"
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
          control={control}
          name={`${hlsPath}.playlist_config.adaptive_refresh_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between">
              <div className="space-y-0.5">
                <FormLabel className="text-xs font-medium">
                  <Trans>Adaptive Refresh (Default: On)</Trans>
                </FormLabel>
                <FormDescription className="text-[10px]">
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
            control={control}
            name={`${hlsPath}.playlist_config.adaptive_refresh_min_interval_ms`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Min Interval (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-7 text-xs font-mono"
                    placeholder="Default: 500"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name={`${hlsPath}.playlist_config.adaptive_refresh_max_interval_ms`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Max Interval (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-7 text-xs font-mono"
                    placeholder="Default: 3000"
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
));
HlsPlaylistSettings.displayName = 'HlsPlaylistSettings';

const HlsSchedulerSettings = React.memo(
  ({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          control={control}
          name={`${hlsPath}.scheduler_config.download_concurrency`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-xs font-semibold">
                <Trans>Download Concurrency</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 5"
                />
              </FormControl>
              <FormDescription className="text-[10px]">
                <Trans>Maximum number of concurrent segment downloads.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={control}
          name={`${hlsPath}.scheduler_config.processed_segment_buffer_multiplier`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-xs">
                <Trans>Processed Buffer Multiplier</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 4"
                />
              </FormControl>
              <FormDescription className="text-[10px]">
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
  ),
);
HlsSchedulerSettings.displayName = 'HlsSchedulerSettings';

const HlsFetcherSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.segment_download_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Segment Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.max_segment_retries`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Segment Retries</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 3"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.segment_retry_delay_base_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Retry Delay Base (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 500"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.max_segment_retry_delay_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Retry Delay (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.streaming_threshold_bytes`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Streaming Threshold (bytes)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 2097152 (2 MiB)"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="space-y-2">
      <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        <Trans>Decryption Keys</Trans>
      </h4>
      <div className="grid gap-4 sm:grid-cols-3">
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.key_download_timeout_ms`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Timeout (ms)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 5000"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.max_key_retries`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Retries</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 3"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.key_retry_delay_base_ms`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Retry Delay (ms)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 200"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.max_key_retry_delay_ms`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Retry Delay (ms)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 5000"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>

    <div className="space-y-2">
      <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        <Trans>Caching</Trans>
      </h4>
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.segment_raw_cache_ttl_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-[10px]">
              <Trans>Raw Segment TTL (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 60000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  </div>
));
HlsFetcherSettings.displayName = 'HlsFetcherSettings';

const HlsProcessorSettings = React.memo(
  ({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
      <FormField
        control={control}
        name={`${hlsPath}.processor_config.processed_segment_ttl_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Processed Segment TTL (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 60000"
              />
            </FormControl>
            <FormDescription className="text-[10px]">
              <Trans>
                How long to keep decrypted/processed segments in cache.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  ),
);
HlsProcessorSettings.displayName = 'HlsProcessorSettings';

const HlsDecryptionSettings = React.memo(
  ({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          control={control}
          name={`${hlsPath}.decryption_config.key_cache_ttl_ms`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-xs">
                <Trans>Key Cache TTL (ms)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 3600000"
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
  ),
);
HlsDecryptionSettings.displayName = 'HlsDecryptionSettings';

const HlsCacheSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-3">
      <FormField
        control={control}
        name={`${hlsPath}.cache_config.playlist_ttl_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Playlist TTL (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 60000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.cache_config.segment_ttl_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Segment TTL (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 120000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.cache_config.decryption_key_ttl_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Decryption Key TTL (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 3600000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  </div>
));
HlsCacheSettings.displayName = 'HlsCacheSettings';

const HlsPerformanceSettings = React.memo(
  ({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
      <div className="grid gap-2">
        <FormField
          control={control}
          name={`${hlsPath}.performance_config.zero_copy_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
              <FormLabel className="text-xs font-normal">
                <Trans>Zero Copy Processing (Default: On)</Trans>
              </FormLabel>
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
          control={control}
          name={`${hlsPath}.performance_config.metrics_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
              <FormLabel className="text-xs font-normal">
                <Trans>Enable Performance Metrics (Default: On)</Trans>
              </FormLabel>
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
      </div>

      <div className="space-y-3">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground border-b border-border/40 pb-1">
          <Trans>Batch Scheduler</Trans>
        </h4>
        <FormField
          control={control}
          name={`${hlsPath}.performance_config.batch_scheduler.enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center gap-2 space-y-0">
              <FormControl>
                <Switch
                  checked={field.value ?? true}
                  onCheckedChange={field.onChange}
                  className="scale-75"
                />
              </FormControl>
              <FormLabel className="text-xs font-normal">
                <Trans>Enabled (Default: On)</Trans>
              </FormLabel>
            </FormItem>
          )}
        />
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.batch_scheduler.batch_window_ms`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px]">
                  <Trans>Batch Window (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
                    placeholder="Default: 50"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.batch_scheduler.max_batch_size`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px]">
                  <Trans>Max Batch Size</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
                    placeholder="Default: 5"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
      </div>

      <div className="space-y-3">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground border-b border-border/40 pb-1">
          <Trans>Prefetching</Trans>
        </h4>
        <div className="flex gap-4">
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.prefetch.enabled`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center gap-2 space-y-0">
                <FormControl>
                  <Switch
                    checked={field.value ?? false}
                    onCheckedChange={field.onChange}
                    className="scale-75"
                  />
                </FormControl>
                <FormLabel className="text-xs font-normal">
                  <Trans>Enabled (Default: Off)</Trans>
                </FormLabel>
              </FormItem>
            )}
          />
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.prefetch.prefetch_count`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px]">
                  <Trans>Prefetch Count</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
                    placeholder="Default: 2"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.prefetch.max_buffer_before_skip`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px]">
                  <Trans>Max Buffer Before Skip</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
                    placeholder="Default: 40"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
      </div>
    </div>
  ),
);
HlsPerformanceSettings.displayName = 'HlsPerformanceSettings';

const HlsOutputSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.output_config.live_reorder_buffer_duration_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Reorder Duration (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.output_config.live_reorder_buffer_max_segments`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Reorder Max Segments</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.output_config.gap_evaluation_interval_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Gap Eval Interval (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 200"
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
      control={control}
      name={`${hlsPath}.output_config.max_pending_init_segments`}
      render={({ field }) => (
        <FormItem>
          <FormLabel className="text-xs">
            <Trans>Max Pending Init Segments</Trans>
          </FormLabel>
          <FormControl>
            <Input
              type="number"
              {...field}
              className="h-8 text-xs font-mono"
              placeholder="Default: 8"
            />
          </FormControl>
          <FormDescription className="text-[10px]">
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
      control={control}
      name={`${hlsPath}.output_config.metrics_enabled`}
      render={({ field }) => (
        <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
          <FormLabel className="text-[11px] font-normal">
            <Trans>Enable Output Metrics (Default: On)</Trans>
          </FormLabel>
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
          control={control}
          name={`${hlsPath}.output_config.buffer_limits.max_segments`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Segments</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 50"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.output_config.buffer_limits.max_bytes`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Bytes</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 104857600 (100 MiB)"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  </div>
));
HlsOutputSettings.displayName = 'HlsOutputSettings';

interface MesioHlsFormProps {
  control: Control<any>;
  basePath?: string;
}

export function MesioHlsForm({
  control,
  basePath = 'config',
}: MesioHlsFormProps) {
  const hlsPath = `${basePath}.hls`;

  return (
    <Card className="border-border/40 bg-background/20 shadow-none overflow-hidden animate-in fade-in slide-in-from-top-1 duration-200">
      <CardContent className="p-3">
        <Tabs defaultValue="base" className="w-full">
          <TabsList className="flex w-full mb-4 bg-muted/30 p-1 py-1.5 h-auto overflow-x-auto no-scrollbar justify-start">
            <TabsTrigger
              value="base"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Globe className="w-3 h-3 text-sky-500" />
              <span className="hidden sm:inline">
                <Trans>Base</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="playlist"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <ListMusic className="w-3 h-3 text-pink-500" />
              <span className="hidden sm:inline">
                <Trans>Playlist</Trans>
              </span>
            </TabsTrigger>

            <TabsTrigger
              value="scheduler"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Zap className="w-3 h-3 text-indigo-500" />
              <span className="hidden sm:inline">
                <Trans>Scheduler</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="fetcher"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Bot className="w-3 h-3 text-purple-500" />
              <span className="hidden sm:inline">
                <Trans>Fetcher</Trans>
              </span>
            </TabsTrigger>

            <TabsTrigger
              value="processor"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Zap className="w-3 h-3 text-orange-500" />
              <span className="hidden sm:inline">
                <Trans>Processor</Trans>
              </span>
            </TabsTrigger>

            <TabsTrigger
              value="decryption"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Zap className="w-3 h-3 text-emerald-500" />
              <span className="hidden sm:inline">
                <Trans>Decryption</Trans>
              </span>
            </TabsTrigger>

            <TabsTrigger
              value="cache"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Zap className="w-3 h-3 text-sky-500" />
              <span className="hidden sm:inline">
                <Trans>Cache</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="performance"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Zap className="w-3 h-3 text-yellow-500" />
              <span className="hidden sm:inline">
                <Trans>Perf</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="output"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Share2 className="w-3 h-3 text-emerald-500" />
              <span className="hidden sm:inline">
                <Trans>Output</Trans>
              </span>
            </TabsTrigger>
          </TabsList>

          <TabsContent value="base" className="mt-0">
            <HlsBaseSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="playlist" className="mt-0">
            <HlsPlaylistSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="scheduler" className="mt-0">
            <HlsSchedulerSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="fetcher" className="mt-0">
            <HlsFetcherSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="processor" className="mt-0">
            <HlsProcessorSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="decryption" className="mt-0">
            <HlsDecryptionSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="cache" className="mt-0">
            <HlsCacheSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="performance" className="mt-0">
            <HlsPerformanceSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="output" className="mt-0">
            <HlsOutputSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  );
}
