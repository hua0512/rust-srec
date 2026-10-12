import React from 'react';
import type { I18n, MessageDescriptor } from '@lingui/core';
import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
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
import { useRowKeys } from '@/hooks/use-row-keys';
import {
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { msg, plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';
import { cn } from '@/lib/utils';
import { Card, CardContent } from '@/components/ui/card';
import {
  INHERIT,
  OverrideStateItems,
  RESET,
  SettingInput,
  SettingSelect,
  SettingSwitch,
  SettingGrid,
  OVERRIDE_SELECT_TRIGGER,
  describeValue,
  overrideMode,
  useEngineOverride,
  useSettingValue,
  type SettingOption,
} from './setting-fields';

interface SubFormProps {
  hlsPath: string;
}

/** Row styling of the on/off settings. */
const SWITCH_ROW =
  'rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm';

function SettingCard({
  label,
  labelId,
  description,
  children,
}: {
  label: React.ReactNode;
  labelId?: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <div id={labelId} className="text-xs font-medium">
          {label}
        </div>
        {children}
        {description && (
          <div className="text-[10px] text-muted-foreground">{description}</div>
        )}
      </CardContent>
    </Card>
  );
}

function describePairs(i18n: I18n, pairs: unknown): string {
  const count = Array.isArray(pairs) ? pairs.length : 0;
  return count === 0
    ? i18n._(msg`None`)
    : i18n._(
        msg`${plural(count, { one: '# parameter', other: '# parameters' })}`,
      );
}

function KeyValuePairsEditor({
  label,
  description,
  path,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  path: string;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const raw = useSettingValue(path) as
    | Array<[string, string]>
    | null
    | undefined;
  const value = raw ?? [];
  const rowKeys = useRowKeys(value.length);
  const mode = overrideMode(raw);

  const addEntry = () => {
    setField(path, [...value, ['', '']]);
  };

  const updateEntry = (idx: number, next: [string, string]) => {
    setField(
      path,
      value.map((pair, i) => (i === idx ? next : pair)),
    );
  };

  const removeEntry = (idx: number) => {
    rowKeys.removeAt(idx);
    const nextValue = value.filter((_, i) => i !== idx);
    // In an override, an empty list replaces the engine's parameters with none.
    setField(path, nextValue.length > 0 || isOverride ? nextValue : undefined);
  };

  const editor = (
    <div className="space-y-2">
      {value.length === 0 && (
        <div className="text-[10px] text-muted-foreground">
          <Trans>No parameters configured.</Trans>
        </div>
      )}

      {value.map(([k, v], idx) => (
        <div
          key={rowKeys.keyAt(idx)}
          className="grid grid-cols-1 sm:grid-cols-5 gap-2"
        >
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
  );

  if (!isOverride) {
    return (
      <SettingCard label={label} description={description}>
        {editor}
      </SettingCard>
    );
  }

  const engineValue = inherited(path);
  return (
    <SettingCard label={label} labelId={labelId} description={description}>
      <Select
        value={mode}
        onValueChange={(next) =>
          setField(
            path,
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : Array.isArray(engineValue)
                  ? engineValue.map((pair) => [...pair])
                  : [],
          )
        }
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={OVERRIDE_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          <OverrideStateItems
            inherited={describePairs(i18n, engineValue)}
            fallback={describePairs(i18n, [])}
          />
          <SelectItem value="custom">
            <Trans>Custom</Trans>
          </SelectItem>
        </SelectContent>
      </Select>
      {mode === 'custom' && editor}
    </SettingCard>
  );
}

/**
 * A timeout an engine can also disable, which it stores as `null`. A template override cannot:
 * the backend merge reads `null` there as "use the default", so an override offers the usual
 * override states instead.
 */
function DisableableDurationField({
  label,
  description,
  path,
  fallback,
  seed,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  path: string;
  /** The engine default: a duration in ms, or `null` when it is disabled by default. */
  fallback: number | null;
  /** The value a custom duration starts from. */
  seed: number;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const raw = useSettingValue(path) as unknown;
  // An emptied input holds `''`, which is unset, not disabled.
  const value = raw === '' ? undefined : raw;
  const custom = value !== undefined && value !== null;

  let control: React.ReactNode;
  if (isOverride) {
    const engineValue = inherited(path);
    control = (
      <Select
        value={overrideMode(value)}
        onValueChange={(next) =>
          setField(
            path,
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : typeof engineValue === 'number'
                  ? engineValue
                  : seed,
          )
        }
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={OVERRIDE_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          <OverrideStateItems
            inherited={describeValue(
              i18n,
              engineValue === undefined ? fallback : engineValue,
            )}
            fallback={describeValue(i18n, fallback)}
          />
          <SelectItem value="custom">
            <Trans>Custom</Trans>
          </SelectItem>
        </SelectContent>
      </Select>
    );
  } else {
    control = (
      <Select
        value={value === null ? 'disabled' : custom ? 'custom' : 'default'}
        onValueChange={(next) => {
          if (next === 'default') setField(path, undefined);
          else if (next === 'disabled') setField(path, null);
          else if (!custom) setField(path, seed);
        }}
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={OVERRIDE_SELECT_TRIGGER}
        >
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
    );
  }

  return (
    <SettingCard
      label={label}
      labelId={labelId}
      description={
        isOverride ? (
          <>
            {description}{' '}
            <Trans>
              A template can't turn this off; do that on the engine.
            </Trans>
          </>
        ) : (
          description
        )
      }
    >
      {control}
      {custom && (
        <Input
          type="number"
          value={
            typeof value === 'number' || typeof value === 'string' ? value : ''
          }
          aria-labelledby={labelId}
          onChange={(e) =>
            setField(path, e.target.value === '' ? undefined : e.target.value)
          }
          className={cn(CONFIG_INPUT, 'font-mono')}
          placeholder={String(seed)}
        />
      )}
    </SettingCard>
  );
}

interface PolicyOption {
  type: string;
  label: MessageDescriptor;
  /** The value written when this option is chosen. */
  value: Record<string, unknown>;
}

/** A policy payload value as text, or `undefined` when it is not a plain value. */
function scalar(value: unknown): string | undefined {
  if (typeof value === 'number') return String(value);
  return typeof value === 'string' && value !== '' ? value : undefined;
}

function describePolicy(
  i18n: I18n,
  options: readonly PolicyOption[],
  policy: unknown,
): string {
  if (!policy || typeof policy !== 'object') return describeValue(i18n, policy);
  const fields = policy as Record<string, unknown>;
  const option = options.find((candidate) => candidate.type === fields.type);
  const label = option ? i18n._(option.label) : (scalar(fields.type) ?? '');
  const duration = scalar(fields.duration_ms);
  const width = scalar(fields.width);
  const details = [
    scalar(fields.count),
    duration === undefined ? undefined : `${duration} ms`,
    scalar(fields.target_bitrate),
    width === undefined ? undefined : `${width}×${scalar(fields.height) ?? ''}`,
    scalar(fields.value),
  ].filter((detail) => detail !== undefined);
  return details.length > 0 ? `${label} (${details.join(', ')})` : label;
}

/**
 * A select for a tagged policy object (`{ type, ...payload }`). In an engine, "Default" leaves the
 * field unset; in an override, the field can also inherit the engine's choice.
 */
function PolicyField({
  label,
  path,
  options,
  fallback,
  renderPayload,
}: {
  label: React.ReactNode;
  path: string;
  options: readonly PolicyOption[];
  /** The policy in effect when the field is unset. */
  fallback: Record<string, unknown>;
  renderPayload: (type: string) => React.ReactNode;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const value = useSettingValue(path) as { type?: string } | null | undefined;
  const isPolicy = value !== null && typeof value === 'object';
  const selected = isPolicy
    ? (value.type ?? RESET)
    : isOverride
      ? overrideMode(value)
      : RESET;

  const select = (next: string) => {
    const option = options.find((candidate) => candidate.type === next);
    if (option) setField(path, { ...option.value });
    else if (next === RESET && isOverride) setField(path, null);
    else setField(path, undefined);
  };

  return (
    <SettingCard label={label} labelId={labelId}>
      <Select value={selected} onValueChange={select}>
        <SelectTrigger
          aria-labelledby={labelId}
          className={OVERRIDE_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          {isOverride ? (
            <OverrideStateItems
              inherited={describePolicy(
                i18n,
                options,
                inherited(path) ?? fallback,
              )}
              fallback={describePolicy(i18n, options, fallback)}
            />
          ) : (
            <SelectItem value={RESET}>
              <Trans>Default</Trans>
            </SelectItem>
          )}
          {options.map((option) => (
            <SelectItem key={option.type} value={option.type}>
              {i18n._(option.label)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {isPolicy && renderPayload(selected)}
    </SettingCard>
  );
}

function PayloadInput({
  name,
  label,
  placeholder,
  numeric = true,
}: {
  name: string;
  label: React.ReactNode;
  placeholder?: string;
  numeric?: boolean;
}) {
  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FormItem className="space-y-2">
          <ConfigFieldLabel size="sm">{label}</ConfigFieldLabel>
          <FormControl>
            <Input
              type={numeric ? 'number' : 'text'}
              {...field}
              value={field.value ?? ''}
              className={cn(CONFIG_INPUT, 'font-mono')}
              placeholder={placeholder}
            />
          </FormControl>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

const GAP_STRATEGY_OPTIONS: readonly PolicyOption[] = [
  {
    type: 'wait_indefinitely',
    label: msg`Wait indefinitely`,
    value: { type: 'wait_indefinitely' },
  },
  {
    type: 'skip_after_count',
    label: msg`Skip after count`,
    value: { type: 'skip_after_count', count: 10 },
  },
  {
    type: 'skip_after_duration',
    label: msg`Skip after duration`,
    value: { type: 'skip_after_duration', duration_ms: 5000 },
  },
  {
    type: 'skip_after_both',
    label: msg`Skip after both`,
    value: { type: 'skip_after_both', count: 10, duration_ms: 5000 },
  },
];

function GapSkipStrategyField({
  label,
  path,
  fallback,
}: {
  label: React.ReactNode;
  path: string;
  fallback: Record<string, unknown>;
}) {
  return (
    <PolicyField
      label={label}
      path={path}
      options={GAP_STRATEGY_OPTIONS}
      fallback={fallback}
      renderPayload={(type) => {
        const count = (
          <PayloadInput
            name={`${path}.count`}
            label={<Trans>Count</Trans>}
            placeholder="10"
          />
        );
        const duration = (
          <PayloadInput
            name={`${path}.duration_ms`}
            label={<Trans>Duration (ms)</Trans>}
            placeholder="5000"
          />
        );
        if (type === 'skip_after_count') return count;
        if (type === 'skip_after_duration') return duration;
        if (type === 'skip_after_both') {
          return (
            <div className="grid gap-3 sm:grid-cols-2">
              {count}
              {duration}
            </div>
          );
        }
        return null;
      }}
    />
  );
}

const VARIANT_POLICY_OPTIONS: readonly PolicyOption[] = [
  {
    type: 'highest_bitrate',
    label: msg`Highest bitrate`,
    value: { type: 'highest_bitrate' },
  },
  {
    type: 'lowest_bitrate',
    label: msg`Lowest bitrate`,
    value: { type: 'lowest_bitrate' },
  },
  {
    type: 'closest_to_bitrate',
    label: msg`Closest to bitrate`,
    value: { type: 'closest_to_bitrate', target_bitrate: 0 },
  },
  { type: 'audio_only', label: msg`Audio only`, value: { type: 'audio_only' } },
  { type: 'video_only', label: msg`Video only`, value: { type: 'video_only' } },
  {
    type: 'matching_resolution',
    label: msg`Matching resolution`,
    value: { type: 'matching_resolution', width: 1920, height: 1080 },
  },
  { type: 'custom', label: msg`Custom`, value: { type: 'custom', value: '' } },
];

function VariantSelectionPolicyField({
  label,
  path,
}: {
  label: React.ReactNode;
  path: string;
}) {
  return (
    <PolicyField
      label={label}
      path={path}
      options={VARIANT_POLICY_OPTIONS}
      fallback={{ type: 'highest_bitrate' }}
      renderPayload={(type) => {
        if (type === 'closest_to_bitrate') {
          return (
            <PayloadInput
              name={`${path}.target_bitrate`}
              label={<Trans>Target bitrate</Trans>}
            />
          );
        }
        if (type === 'matching_resolution') {
          return (
            <div className="grid gap-3 sm:grid-cols-2">
              <PayloadInput
                name={`${path}.width`}
                label={<Trans>Width</Trans>}
              />
              <PayloadInput
                name={`${path}.height`}
                label={<Trans>Height</Trans>}
              />
            </div>
          );
        }
        if (type === 'custom') {
          return (
            <PayloadInput
              name={`${path}.value`}
              label={<Trans>Value</Trans>}
              numeric={false}
            />
          );
        }
        return null;
      }}
    />
  );
}

const HTTP_VERSION_OPTIONS: readonly SettingOption[] = [
  {
    value: 'auto',
    label: msg`Auto`,
    content: <Trans>Auto (Default)</Trans>,
  },
  { value: 'http2_only', label: msg`HTTP/2 Only` },
  { value: 'http1_only', label: msg`HTTP/1.1 Only` },
];

const HlsBaseSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.base`;
  return (
    <div className="space-y-4">
      <SettingGrid>
        <SettingInput
          name={`${path}.timeout_ms`}
          label={<Trans>Global Timeout (ms)</Trans>}
          fallback={0}
          fallbackLabel="0, no timeout"
        />
        <SettingInput
          name={`${path}.connect_timeout_ms`}
          label={<Trans>Connect Timeout (ms)</Trans>}
          fallback={30000}
        />
        <SettingInput
          name={`${path}.read_timeout_ms`}
          label={<Trans>Read Timeout (ms)</Trans>}
          fallback={30000}
        />
        <SettingInput
          name={`${path}.write_timeout_ms`}
          label={<Trans>Write Timeout (ms)</Trans>}
          fallback={30000}
        />
      </SettingGrid>

      <KeyValuePairsEditor
        label={<Trans>Query Parameters</Trans>}
        description={
          <Trans>
            Appended to all HLS requests. Useful for signed URLs or CDN routing.
          </Trans>
        }
        path={`${path}.params`}
      />

      <SettingGrid>
        <SettingInput
          name={`${path}.user_agent`}
          label={<Trans>User Agent</Trans>}
          numeric={false}
          fallback=""
          fallbackLabel="Mozilla/5.0..."
        />
        <SettingSelect
          name={`${path}.http_version`}
          label={<Trans>HTTP Version Preference</Trans>}
          options={HTTP_VERSION_OPTIONS}
          fallback="auto"
        />
      </SettingGrid>

      <SettingGrid>
        <SettingInput
          name={`${path}.http2_keep_alive_interval_ms`}
          label={<Trans>H2 Keep-Alive (ms)</Trans>}
          fallback={20000}
        />
        <SettingInput
          name={`${path}.pool_max_idle_per_host`}
          label={<Trans>Max Idle per Host</Trans>}
          fallback={10}
        />
        <SettingInput
          name={`${path}.pool_idle_timeout_ms`}
          label={<Trans>Pool Idle Timeout (ms)</Trans>}
          fallback={30000}
        />
      </SettingGrid>

      <SettingGrid>
        <SettingSwitch
          name={`${path}.follow_redirects`}
          label={<Trans>Follow Redirects (Default: On)</Trans>}
          fallback={true}
          className={SWITCH_ROW}
        />
        <SettingSwitch
          name={`${path}.danger_accept_invalid_certs`}
          label={<Trans>Accept Invalid Certs (Default: Off)</Trans>}
          fallback={false}
          className={SWITCH_ROW}
        />
        <SettingSwitch
          name={`${path}.force_ipv4`}
          label={<Trans>Force IPv4 (Default: Off)</Trans>}
          fallback={false}
          className={SWITCH_ROW}
        />
        <SettingSwitch
          name={`${path}.force_ipv6`}
          label={<Trans>Force IPv6 (Default: Off)</Trans>}
          fallback={false}
          className={SWITCH_ROW}
        />
      </SettingGrid>
    </div>
  );
});
HlsBaseSettings.displayName = 'HlsBaseSettings';

const HlsPlaylistSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.playlist_config`;
  return (
    <div className="space-y-4">
      <SettingGrid>
        <SettingInput
          name={`${path}.initial_playlist_fetch_timeout_ms`}
          label={<Trans>Initial Fetch Timeout (ms)</Trans>}
          fallback={15000}
        />
        <SettingInput
          name={`${path}.live_refresh_interval_ms`}
          label={<Trans>Live Refresh Interval (ms)</Trans>}
          fallback={1000}
        />
        <SettingInput
          name={`${path}.live_max_refresh_retries`}
          label={<Trans>Max Refresh Retries</Trans>}
          fallback={5}
        />
        <SettingInput
          name={`${path}.live_refresh_retry_delay_ms`}
          label={<Trans>Retry Delay (ms)</Trans>}
          fallback={1000}
        />
      </SettingGrid>

      <VariantSelectionPolicyField
        label={<Trans>Variant Selection Policy</Trans>}
        path={`${path}.variant_selection_policy`}
      />

      <Card className="border-border/40 bg-muted/5">
        <CardContent className="p-3 space-y-3">
          <SettingSwitch
            name={`${path}.adaptive_refresh_enabled`}
            label={<Trans>Adaptive Refresh (Default: On)</Trans>}
            description={<Trans>Adjust rate based on target duration</Trans>}
            fallback={true}
            size="default"
          />

          <SettingGrid className="pt-2 border-t border-border/40">
            <SettingInput
              name={`${path}.adaptive_refresh_min_interval_ms`}
              label={<Trans>Min Interval (ms)</Trans>}
              fallback={500}
              size="sm"
            />
            <SettingInput
              name={`${path}.adaptive_refresh_max_interval_ms`}
              label={<Trans>Max Interval (ms)</Trans>}
              fallback={3000}
              size="sm"
            />
          </SettingGrid>
        </CardContent>
      </Card>
    </div>
  );
});
HlsPlaylistSettings.displayName = 'HlsPlaylistSettings';

const HlsSchedulerSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.scheduler_config`;
  return (
    <SettingGrid>
      <SettingInput
        name={`${path}.download_concurrency`}
        label={<Trans>Download Concurrency</Trans>}
        description={
          <Trans>Maximum number of concurrent segment downloads.</Trans>
        }
        fallback={5}
      />
      <SettingInput
        name={`${path}.processed_segment_buffer_multiplier`}
        label={<Trans>Processed Buffer Multiplier</Trans>}
        description={
          <Trans>Channel buffer size multiplier for processed segments.</Trans>
        }
        fallback={4}
      />
    </SettingGrid>
  );
});
HlsSchedulerSettings.displayName = 'HlsSchedulerSettings';

const HlsFetcherSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.fetcher_config`;
  return (
    <SettingGrid>
      <SettingInput
        name={`${path}.segment_download_timeout_ms`}
        label={<Trans>Segment Timeout (ms)</Trans>}
        fallback={10000}
      />
      <SettingInput
        name={`${path}.max_segment_retries`}
        label={<Trans>Max Segment Retries</Trans>}
        fallback={3}
      />
      <SettingInput
        name={`${path}.segment_retry_delay_base_ms`}
        label={<Trans>Retry Delay Base (ms)</Trans>}
        fallback={500}
      />
      <SettingInput
        name={`${path}.max_segment_retry_delay_ms`}
        label={<Trans>Max Retry Delay (ms)</Trans>}
        fallback={10000}
      />
      <SettingInput
        name={`${path}.key_download_timeout_ms`}
        label={<Trans>Key Timeout (ms)</Trans>}
        fallback={5000}
        size="sm"
      />
      <SettingInput
        name={`${path}.max_key_retries`}
        label={<Trans>Max Key Retries</Trans>}
        fallback={3}
        size="sm"
      />
      <SettingInput
        name={`${path}.key_retry_delay_base_ms`}
        label={<Trans>Key Retry Delay (ms)</Trans>}
        fallback={200}
        size="sm"
      />
      <SettingInput
        name={`${path}.max_key_retry_delay_ms`}
        label={<Trans>Max Key Retry Delay (ms)</Trans>}
        fallback={5000}
        size="sm"
      />
    </SettingGrid>
  );
});
HlsFetcherSettings.displayName = 'HlsFetcherSettings';

const HlsProcessorSettings = React.memo(({ hlsPath }: SubFormProps) => (
  <SettingInput
    name={`${hlsPath}.processor_config.processed_segment_ttl_ms`}
    label={<Trans>Processed Segment TTL (ms)</Trans>}
    description={
      <Trans>How long to keep decrypted/processed segments in cache.</Trans>
    }
    fallback={60000}
  />
));
HlsProcessorSettings.displayName = 'HlsProcessorSettings';

const HlsDecryptionSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.decryption_config`;
  return (
    <SettingGrid>
      <SettingInput
        name={`${path}.key_cache_ttl_ms`}
        label={<Trans>Key Cache TTL (ms)</Trans>}
        fallback={3600000}
      />
      <SettingSwitch
        name={`${path}.offload_decryption_to_cpu_pool`}
        label={<Trans>Offload Decryption (Default: On)</Trans>}
        description={
          <Trans>
            Runs decryption on a blocking thread pool to avoid stalling async
            tasks.
          </Trans>
        }
        fallback={true}
        className={SWITCH_ROW}
      />
    </SettingGrid>
  );
});
HlsDecryptionSettings.displayName = 'HlsDecryptionSettings';

const HlsCacheSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.cache_config`;
  return (
    <SettingGrid>
      <SettingInput
        name={`${path}.playlist_ttl_ms`}
        label={<Trans>Playlist TTL (ms)</Trans>}
        fallback={60000}
      />
      <SettingInput
        name={`${path}.segment_ttl_ms`}
        label={<Trans>Segment TTL (ms)</Trans>}
        fallback={120000}
      />
      <SettingInput
        name={`${path}.decryption_key_ttl_ms`}
        label={<Trans>Decryption Key TTL (ms)</Trans>}
        fallback={3600000}
      />
    </SettingGrid>
  );
});
HlsCacheSettings.displayName = 'HlsCacheSettings';

const HlsOutputSettings = React.memo(({ hlsPath }: SubFormProps) => {
  const path = `${hlsPath}.output_config`;
  return (
    <div className="space-y-4">
      <SettingGrid>
        <SettingInput
          name={`${path}.live_reorder_buffer_duration_ms`}
          label={<Trans>Reorder Duration (ms)</Trans>}
          fallback={30000}
        />
        <SettingInput
          name={`${path}.live_reorder_buffer_max_segments`}
          label={<Trans>Reorder Max Segments</Trans>}
          fallback={10}
        />
        <SettingInput
          name={`${path}.gap_evaluation_interval_ms`}
          label={<Trans>Gap Eval Interval (ms)</Trans>}
          fallback={200}
        />
        <SettingInput
          name={`${path}.max_pending_init_segments`}
          label={<Trans>Max Pending Init Segments</Trans>}
          description={<Trans>0 disables the limit.</Trans>}
          fallback={8}
        />
      </SettingGrid>

      <SettingGrid>
        <GapSkipStrategyField
          label={<Trans>Live Gap Strategy</Trans>}
          path={`${path}.live_gap_strategy`}
          fallback={{ type: 'skip_after_both', count: 10, duration_ms: 5000 }}
        />
        <GapSkipStrategyField
          label={<Trans>VOD Gap Strategy</Trans>}
          path={`${path}.vod_gap_strategy`}
          fallback={{ type: 'wait_indefinitely' }}
        />
      </SettingGrid>

      {/* The two timeouts an engine can disable share a row of their own. */}
      <SettingGrid>
        <DisableableDurationField
          label={<Trans>Max Stall Duration (ms)</Trans>}
          description={
            <Trans>
              Default uses Mesio’s built-in live stall timeout. Disabled means
              wait indefinitely.
            </Trans>
          }
          path={`${path}.live_max_overall_stall_duration_ms`}
          fallback={60000}
          seed={60000}
        />
        <DisableableDurationField
          label={<Trans>VOD Segment Timeout (ms)</Trans>}
          description={
            <Trans>
              When enabled, each VOD segment must complete within this timeout.
            </Trans>
          }
          path={`${path}.vod_segment_timeout_ms`}
          fallback={null}
          seed={30000}
        />
      </SettingGrid>

      <SettingSwitch
        name={`${path}.metrics_enabled`}
        label={<Trans>Enable Output Metrics (Default: On)</Trans>}
        fallback={true}
        className={SWITCH_ROW}
      />

      <div className="space-y-3">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground border-b border-border/40 pb-1">
          <Trans>Buffer Limits</Trans>
        </h4>
        <SettingGrid>
          <SettingInput
            name={`${path}.buffer_limits.max_segments`}
            label={<Trans>Max Segments</Trans>}
            fallback={50}
            size="sm"
          />
          <SettingInput
            name={`${path}.buffer_limits.max_bytes`}
            label={<Trans>Max Bytes</Trans>}
            fallback={104857600}
            fallbackLabel="104857600, 100 MiB"
            size="sm"
          />
        </SettingGrid>
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
