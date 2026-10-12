import React from 'react';
import { useFormContext, useFormState, useWatch } from 'react-hook-form';
import type { I18n, MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { CalendarClock, FileText, Layers, Radio } from 'lucide-react';
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
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';
import { Card, CardContent } from '@/components/ui/card';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { useDefaultPlaceholder } from '@/hooks/use-default-placeholder';
import {
  MESIO_DASH_DEFAULTS,
  type MesioDashSettings,
} from '@/api/schemas/engine';
import { cn } from '@/lib/utils';

/** Fields sit side by side only when the form is wide enough for both. */
const FIELD_GRID =
  'grid gap-4 grid-cols-[repeat(auto-fit,minmax(min(100%,14rem),1fr))]';

type SettingKey = keyof MesioDashSettings;

/**
 * Where the fields live, and for a template override the engine's settings it inherits.
 *
 * In an override, an absent field inherits the engine's value and `null` resets it to the engine
 * default (the backend merge removes null-valued keys), so every field offers both states.
 */
interface DashFormContextValue {
  dashPath: string;
  inherited: MesioDashSettings | null;
}

const DashFormContext = React.createContext<DashFormContextValue>({
  dashPath: 'config.dash',
  inherited: null,
});

/** Select values for the two non-value states of an override field. */
const INHERIT = 'inherit';
const RESET = 'reset';

type OverrideMode = typeof INHERIT | typeof RESET | 'custom';

function overrideMode(value: unknown): OverrideMode {
  if (value === undefined) return INHERIT;
  if (value === null) return RESET;
  return 'custom';
}

interface PolicyOption {
  type: string;
  label: MessageDescriptor;
  /** The value written when this option is chosen. */
  value: Record<string, unknown>;
}

function describePolicy(
  i18n: I18n,
  options: PolicyOption[],
  policy: Record<string, unknown>,
): string {
  const option = options.find((candidate) => candidate.type === policy.type);
  const label = option ? i18n._(option.label) : String(policy.type);
  if (typeof policy.height === 'number') return `${label} (${policy.height}px)`;
  if (typeof policy.bandwidth === 'number')
    return `${label} (${policy.bandwidth} bit/s)`;
  if (typeof policy.delay_ms === 'number')
    return `${label} (${policy.delay_ms} ms)`;
  return label;
}

function describeSetting(
  i18n: I18n,
  value: string | number | boolean | null | undefined,
): string {
  if (value === true) return i18n._(msg`On`);
  if (value === false) return i18n._(msg`Off`);
  if (value === undefined || value === '') return i18n._(msg`Any language`);
  return String(value);
}

/** The two non-value choices an override field offers, labelled with what they resolve to. */
function OverrideStateItems({
  inherited,
  fallback,
}: {
  inherited: string;
  fallback: string;
}) {
  const { i18n } = useLingui();
  return (
    <>
      <SelectItem value={INHERIT}>
        {i18n._(msg`Use engine setting (${inherited})`)}
      </SelectItem>
      <SelectItem value={RESET}>
        {i18n._(msg`Default (${fallback})`)}
      </SelectItem>
    </>
  );
}

function FieldCard({
  labelId,
  label,
  description,
  children,
}: {
  labelId: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <div className="space-y-0.5">
          <div id={labelId} className="text-xs font-medium">
            {label}
          </div>
          {description && (
            <div className="text-[10px] text-muted-foreground">
              {description}
            </div>
          )}
        </div>
        {children}
      </CardContent>
    </Card>
  );
}

/**
 * A select for a tagged policy object (`{ type, ...payload }`). In an engine, "Default" leaves the
 * field unset; in an override, the field can also inherit the engine's choice.
 */
function PolicyField({
  setting,
  label,
  description,
  options,
  renderPayload,
}: {
  setting: 'video_quality' | 'audio_quality' | 'live_start';
  label: React.ReactNode;
  description: React.ReactNode;
  options: PolicyOption[];
  renderPayload: (type: string) => React.ReactNode;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { dashPath, inherited } = React.useContext(DashFormContext);
  const path = `${dashPath}.${setting}`;
  const { setValue, control } = useFormContext<any>();
  const value = useWatch({ control, name: path }) as
    | { type?: string }
    | null
    | undefined;
  const selected =
    value && typeof value === 'object'
      ? (value.type ?? RESET)
      : inherited
        ? overrideMode(value)
        : RESET;

  const select = (next: string) => {
    const option = options.find((candidate) => candidate.type === next);
    let nextValue: unknown;
    if (option) nextValue = { ...option.value };
    else if (next === RESET && inherited) nextValue = null;
    setValue(path, nextValue, { shouldDirty: true, shouldValidate: true });
  };

  return (
    <FieldCard labelId={labelId} label={label} description={description}>
      <Select value={selected} onValueChange={select}>
        <SelectTrigger
          aria-labelledby={labelId}
          className={CONFIG_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          {inherited ? (
            <OverrideStateItems
              inherited={describePolicy(i18n, options, inherited[setting])}
              fallback={describePolicy(
                i18n,
                options,
                MESIO_DASH_DEFAULTS[setting],
              )}
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
      {value && typeof value === 'object' && renderPayload(selected)}
    </FieldCard>
  );
}

function NumberInput({
  name,
  label,
  description,
  placeholder,
}: {
  name: string;
  label?: React.ReactNode;
  description?: React.ReactNode;
  placeholder?: string;
}) {
  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FormItem className="space-y-2">
          {label && <ConfigFieldLabel size="sm">{label}</ConfigFieldLabel>}
          <FormControl>
            <Input
              type="number"
              {...field}
              value={field.value ?? ''}
              className={cn(CONFIG_INPUT, 'font-mono')}
              placeholder={placeholder}
            />
          </FormControl>
          {description && (
            <FormDescription className={CONFIG_DESCRIPTION}>
              {description}
            </FormDescription>
          )}
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

/**
 * A number or text setting. In an engine, an empty input uses the default; in an override, a
 * select chooses between the engine's setting, the default, and a custom value.
 */
function ValueField({
  setting,
  label,
  description,
  numeric = true,
  placeholder,
}: {
  setting: Exclude<
    SettingKey,
    | 'video_quality'
    | 'audio_quality'
    | 'live_start'
    | 'include_video'
    | 'include_audio'
  >;
  label: React.ReactNode;
  description?: React.ReactNode;
  numeric?: boolean;
  placeholder?: string;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const defaultPlaceholder = useDefaultPlaceholder();
  const { dashPath, inherited } = React.useContext(DashFormContext);
  const path = `${dashPath}.${setting}`;
  const { setValue, control } = useFormContext<any>();
  const value = useWatch({ control, name: path });
  // Only the language has no default (any language).
  const fallback =
    setting === 'audio_language' ? undefined : MESIO_DASH_DEFAULTS[setting];
  const mode = overrideMode(value);

  const input = (
    <FormField
      name={path}
      render={({ field }) => (
        <FormItem className="space-y-2">
          <FormControl>
            <Input
              type={numeric ? 'number' : 'text'}
              {...field}
              value={field.value ?? ''}
              aria-labelledby={labelId}
              className={cn(CONFIG_INPUT, 'font-mono')}
              placeholder={
                placeholder ??
                (fallback === undefined
                  ? undefined
                  : defaultPlaceholder(fallback))
              }
            />
          </FormControl>
          <FormMessage />
        </FormItem>
      )}
    />
  );

  if (!inherited) {
    return (
      <FieldCard labelId={labelId} label={label} description={description}>
        {input}
      </FieldCard>
    );
  }

  const selectId = `${labelId}-mode`;
  return (
    <FieldCard labelId={labelId} label={label} description={description}>
      <Select
        value={mode}
        onValueChange={(next) => {
          const nextValue =
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : (inherited[setting] ?? '');
          setValue(path, nextValue, {
            shouldDirty: true,
            shouldValidate: true,
          });
        }}
      >
        <SelectTrigger
          id={selectId}
          aria-labelledby={labelId}
          className={CONFIG_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          <OverrideStateItems
            inherited={describeSetting(i18n, inherited[setting])}
            fallback={describeSetting(i18n, fallback)}
          />
          <SelectItem value="custom">
            <Trans>Custom</Trans>
          </SelectItem>
        </SelectContent>
      </Select>
      {mode === 'custom' && input}
    </FieldCard>
  );
}

/**
 * Whether a track is recorded. Not a registered field: a controller mounted after the `dash`
 * object was `null` in the form's defaults would take `null` from it, silently turning
 * "inherit" into "reset".
 */
function TrackField({
  setting,
  label,
  description,
}: {
  setting: 'include_video' | 'include_audio';
  label: React.ReactNode;
  description?: React.ReactNode;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { dashPath, inherited } = React.useContext(DashFormContext);
  const path = `${dashPath}.${setting}`;
  // The "record at least one track" error is reported on the audio field.
  const errorPath = `${dashPath}.include_audio`;
  const { control, setValue, trigger, getFieldState } = useFormContext<any>();
  const value = useWatch({ control, name: path }) as boolean | null | undefined;
  const formState = useFormState({ control, name: path });
  const error =
    setting === 'include_audio'
      ? getFieldState(path, formState).error
      : undefined;

  const change = (next: boolean | null | undefined) => {
    setValue(path, next, { shouldDirty: true });
    void trigger(errorPath);
  };

  return (
    <div className="space-y-1 rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
      <div className="flex flex-row flex-wrap items-center justify-between gap-3">
        <div className="min-w-0 space-y-0.5">
          <ConfigFieldLabel size="sm">
            <span id={labelId}>{label}</span>
          </ConfigFieldLabel>
          {description && <p className={CONFIG_DESCRIPTION}>{description}</p>}
        </div>
        {inherited ? (
          <Select
            value={
              typeof value === 'boolean' ? String(value) : overrideMode(value)
            }
            onValueChange={(next) =>
              change(
                next === INHERIT
                  ? undefined
                  : next === RESET
                    ? null
                    : next === 'true',
              )
            }
          >
            <SelectTrigger
              aria-labelledby={labelId}
              aria-invalid={error ? true : undefined}
              className={cn(CONFIG_SELECT_TRIGGER, 'sm:w-56')}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={CONFIG_SELECT_CONTENT}>
              <OverrideStateItems
                inherited={describeSetting(i18n, inherited[setting])}
                fallback={describeSetting(i18n, MESIO_DASH_DEFAULTS[setting])}
              />
              <SelectItem value="true">{i18n._(msg`On`)}</SelectItem>
              <SelectItem value="false">{i18n._(msg`Off`)}</SelectItem>
            </SelectContent>
          </Select>
        ) : (
          <Switch
            aria-labelledby={labelId}
            aria-invalid={error ? true : undefined}
            checked={value ?? MESIO_DASH_DEFAULTS[setting]}
            onCheckedChange={change}
            className="scale-75 origin-right"
          />
        )}
      </div>
      {error?.message && (
        <p role="alert" className="text-destructive text-sm">
          {error.message}
        </p>
      )}
    </div>
  );
}

const VIDEO_QUALITY_OPTIONS: PolicyOption[] = [
  {
    type: 'highest',
    label: msg`Highest bandwidth`,
    value: { type: 'highest' },
  },
  { type: 'lowest', label: msg`Lowest bandwidth`, value: { type: 'lowest' } },
  {
    type: 'max_height',
    label: msg`Highest up to a height`,
    value: { type: 'max_height', height: 1080 },
  },
  {
    type: 'closest_bandwidth',
    label: msg`Closest to a bandwidth`,
    value: { type: 'closest_bandwidth', bandwidth: 5000000 },
  },
];

const AUDIO_QUALITY_OPTIONS: PolicyOption[] = [
  {
    type: 'highest',
    label: msg`Highest bandwidth`,
    value: { type: 'highest' },
  },
  { type: 'lowest', label: msg`Lowest bandwidth`, value: { type: 'lowest' } },
  {
    type: 'closest_bandwidth',
    label: msg`Closest to a bandwidth`,
    value: { type: 'closest_bandwidth', bandwidth: 128000 },
  },
];

const LIVE_START_OPTIONS: PolicyOption[] = [
  { type: 'auto', label: msg`Automatic`, value: { type: 'auto' } },
  { type: 'live_edge', label: msg`Live edge`, value: { type: 'live_edge' } },
  {
    type: 'delay',
    label: msg`Fixed delay`,
    value: { type: 'delay', delay_ms: 10000 },
  },
];

function DashTrackSettings() {
  const { i18n } = useLingui();
  const { dashPath } = React.useContext(DashFormContext);
  return (
    <div className="space-y-4">
      <div className={FIELD_GRID}>
        <PolicyField
          setting="video_quality"
          label={<Trans>Video Quality</Trans>}
          description={
            <Trans>Which video version is recorded. Default: highest.</Trans>
          }
          options={VIDEO_QUALITY_OPTIONS}
          renderPayload={(type) =>
            type === 'max_height' ? (
              <NumberInput
                name={`${dashPath}.video_quality.height`}
                label={<Trans>Maximum height (px)</Trans>}
                description={
                  <Trans>The smallest picture is used when none fits.</Trans>
                }
              />
            ) : type === 'closest_bandwidth' ? (
              <NumberInput
                name={`${dashPath}.video_quality.bandwidth`}
                label={<Trans>Target bandwidth (bit/s)</Trans>}
              />
            ) : null
          }
        />
        <PolicyField
          setting="audio_quality"
          label={<Trans>Audio Quality</Trans>}
          description={
            <Trans>
              Which audio version is recorded in the preferred language.
              Default: highest.
            </Trans>
          }
          options={AUDIO_QUALITY_OPTIONS}
          renderPayload={(type) =>
            type === 'closest_bandwidth' ? (
              <NumberInput
                name={`${dashPath}.audio_quality.bandwidth`}
                label={<Trans>Target bandwidth (bit/s)</Trans>}
              />
            ) : null
          }
        />
      </div>

      <ValueField
        setting="audio_language"
        numeric={false}
        label={<Trans>Preferred Audio Language</Trans>}
        description={
          <Trans>
            A language tag. Any language is used when this is empty or no audio
            matches.
          </Trans>
        }
        placeholder={i18n._(msg`e.g. en, pt-BR`)}
      />

      <div className={FIELD_GRID}>
        <TrackField
          setting="include_video"
          label={<Trans>Record Video (Default: On)</Trans>}
        />
        <TrackField
          setting="include_audio"
          label={<Trans>Record Separate Audio (Default: On)</Trans>}
          description={
            <Trans>Audio inside the video stream is always kept.</Trans>
          }
        />
      </div>
    </div>
  );
}

function DashLiveStartSettings() {
  const { dashPath } = React.useContext(DashFormContext);
  return (
    <PolicyField
      setting="live_start"
      label={<Trans>Live Start Position</Trans>}
      description={
        <Trans>
          Where recording of a live stream starts. Automatic follows the delay
          suggested by the stream, else three segments behind the live edge.
          Finished streams are always recorded from the beginning.
        </Trans>
      }
      options={LIVE_START_OPTIONS}
      renderPayload={(type) =>
        type === 'delay' ? (
          <NumberInput
            name={`${dashPath}.live_start.delay_ms`}
            label={<Trans>Delay behind the live edge (ms)</Trans>}
          />
        ) : null
      }
    />
  );
}

function DashDownloadSettings() {
  return (
    <div className={FIELD_GRID}>
      <ValueField
        setting="download_concurrency"
        label={<Trans>Download Concurrency</Trans>}
        description={<Trans>Segments downloaded at once, per track.</Trans>}
      />
      <ValueField
        setting="segment_timeout_ms"
        label={<Trans>Segment Timeout (ms)</Trans>}
      />
      <ValueField
        setting="max_segment_retries"
        label={<Trans>Segment Retries</Trans>}
      />
    </div>
  );
}

function DashManifestSettings() {
  return (
    <div className={FIELD_GRID}>
      <ValueField
        setting="manifest_timeout_ms"
        label={<Trans>Manifest Timeout (ms)</Trans>}
      />
      <ValueField
        setting="min_refresh_interval_ms"
        label={<Trans>Minimum Refresh Interval (ms)</Trans>}
        description={
          <Trans>The manifest's own update period applies when longer.</Trans>
        }
      />
      <ValueField
        setting="max_refresh_retries"
        label={<Trans>Refresh Retries</Trans>}
        description={
          <Trans>
            Failed manifest refreshes in a row before the recording fails.
          </Trans>
        }
      />
    </div>
  );
}

const DASH_SECTIONS = [
  {
    value: 'tracks',
    icon: Layers,
    label: msg`Tracks`,
    Section: DashTrackSettings,
  },
  {
    value: 'live-start',
    icon: Radio,
    label: msg`Live start`,
    Section: DashLiveStartSettings,
  },
  {
    value: 'downloads',
    icon: CalendarClock,
    label: msg`Downloads`,
    Section: DashDownloadSettings,
  },
  {
    value: 'manifest',
    icon: FileText,
    label: msg`Manifest`,
    Section: DashManifestSettings,
  },
];

/**
 * A template override's choice between adjusting single DASH settings and resetting all of them
 * (`"dash": null`). While reset, no DASH field is mounted: a field mounted under the `null`
 * object would turn it back into an object and the reset would be lost on save.
 */
function DashResetChoice({ dashPath }: { dashPath: string }) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { setValue, control } = useFormContext<any>();
  const reset = useWatch({ control, name: dashPath }) === null;

  return (
    <FieldCard
      labelId={labelId}
      label={<Trans>DASH settings in this template</Trans>}
      description={
        reset ? (
          <Trans>
            Every DASH setting uses the engine default, whatever the engine
            sets.
          </Trans>
        ) : undefined
      }
    >
      <Select
        value={reset ? RESET : 'adjust'}
        onValueChange={(next) =>
          setValue(dashPath, next === RESET ? null : undefined, {
            shouldDirty: true,
          })
        }
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={CONFIG_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          <SelectItem value="adjust">
            {i18n._(msg`Change individual settings`)}
          </SelectItem>
          <SelectItem value={RESET}>
            {i18n._(msg`Reset all DASH settings to defaults`)}
          </SelectItem>
        </SelectContent>
      </Select>
    </FieldCard>
  );
}

interface MesioDashFormProps {
  basePath?: string;
  /**
   * For a template override: the DASH settings of the engine it overrides, with unset fields at
   * their defaults. Absent for an engine's own configuration.
   */
  inherited?: MesioDashSettings;
}

/** MPEG-DASH settings of a mesio engine or of a template's override of one. */
export function MesioDashForm({
  basePath = 'config',
  inherited,
}: MesioDashFormProps) {
  const { i18n } = useLingui();
  const dashPath = `${basePath}.dash`;
  const reset =
    useWatch({ name: dashPath }) === null && inherited !== undefined;
  const context = React.useMemo(
    () => ({ dashPath, inherited: inherited ?? null }),
    [dashPath, inherited],
  );

  return (
    <DashFormContext.Provider value={context}>
      <div className="space-y-4">
        {inherited && <DashResetChoice dashPath={dashPath} />}
        {!reset && (
          <Accordion
            type="multiple"
            defaultValue={['tracks']}
            className="w-full"
          >
            {DASH_SECTIONS.map(({ value, icon: Icon, label, Section }) => (
              <AccordionItem
                key={value}
                value={value}
                className="border-border/50"
              >
                <AccordionTrigger className="py-3 hover:no-underline">
                  <span className="flex items-center gap-2.5">
                    <Icon className="h-4 w-4 text-muted-foreground" />
                    <span className="text-sm font-medium">{i18n._(label)}</span>
                  </span>
                </AccordionTrigger>
                <AccordionContent className="pb-4 pt-1">
                  <Section />
                </AccordionContent>
              </AccordionItem>
            ))}
          </Accordion>
        )}
      </div>
    </DashFormContext.Provider>
  );
}
