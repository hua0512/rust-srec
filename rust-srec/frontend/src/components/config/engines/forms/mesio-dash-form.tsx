import React from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import type { MessageDescriptor } from '@lingui/core';
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
import { MESIO_DASH_DEFAULTS } from '@/api/schemas/engine';
import { cn } from '@/lib/utils';

/** Fields sit side by side only when the form is wide enough for both. */
const FIELD_GRID =
  'grid gap-4 grid-cols-[repeat(auto-fit,minmax(min(100%,14rem),1fr))]';

interface SectionProps {
  dashPath: string;
}

interface PolicyOption {
  type: string;
  label: MessageDescriptor;
  /** The value written when this option is chosen. */
  value: Record<string, unknown>;
}

/**
 * A select for a tagged policy object (`{ type, ...payload }`). "Default" leaves the field unset,
 * so the engine default applies (or, in a template override, the engine's own setting).
 */
function PolicyField({
  label,
  description,
  path,
  options,
  renderPayload,
}: {
  label: React.ReactNode;
  description: React.ReactNode;
  path: string;
  options: PolicyOption[];
  renderPayload: (type: string) => React.ReactNode;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { setValue, control } = useFormContext<any>();
  const value = useWatch({ control, name: path }) as
    | { type?: string }
    | null
    | undefined;
  const type = value?.type ?? 'default';

  const setType = (next: string) => {
    const option = options.find((candidate) => candidate.type === next);
    setValue(path, option ? { ...option.value } : undefined, {
      shouldDirty: true,
      shouldValidate: true,
    });
  };

  return (
    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <div className="space-y-0.5">
          <div id={labelId} className="text-xs font-medium">
            {label}
          </div>
          <div className="text-[10px] text-muted-foreground">{description}</div>
        </div>
        <Select value={type} onValueChange={setType}>
          <SelectTrigger
            aria-labelledby={labelId}
            className={CONFIG_SELECT_TRIGGER}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={CONFIG_SELECT_CONTENT}>
            <SelectItem value="default">
              <Trans>Default</Trans>
            </SelectItem>
            {options.map((option) => (
              <SelectItem key={option.type} value={option.type}>
                {i18n._(option.label)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {renderPayload(type)}
      </CardContent>
    </Card>
  );
}

function NumberField({
  name,
  label,
  description,
  placeholder,
}: {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  placeholder?: string;
}) {
  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FormItem className="space-y-2">
          <ConfigFieldLabel size="sm">{label}</ConfigFieldLabel>
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

function TrackSwitch({
  name,
  label,
  description,
  errorPath,
}: {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  /** Where the "record at least one track" error is reported. */
  errorPath: string;
}) {
  const { trigger } = useFormContext();
  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FormItem className="rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
          <div className="flex flex-row items-center justify-between gap-3">
            <div className="min-w-0 space-y-0.5">
              <ConfigFieldLabel size="sm">{label}</ConfigFieldLabel>
              {description && (
                <FormDescription className={CONFIG_DESCRIPTION}>
                  {description}
                </FormDescription>
              )}
            </div>
            <FormControl>
              <Switch
                checked={field.value ?? true}
                onCheckedChange={(checked) => {
                  field.onChange(checked);
                  void trigger(errorPath);
                }}
                className="scale-75 origin-right"
              />
            </FormControl>
          </div>
          <FormMessage />
        </FormItem>
      )}
    />
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

const DashTrackSettings = React.memo(({ dashPath }: SectionProps) => {
  const { i18n } = useLingui();
  return (
    <div className="space-y-4">
      <div className={FIELD_GRID}>
        <PolicyField
          label={<Trans>Video Quality</Trans>}
          description={
            <Trans>Which video version is recorded. Default: highest.</Trans>
          }
          path={`${dashPath}.video_quality`}
          options={VIDEO_QUALITY_OPTIONS}
          renderPayload={(type) =>
            type === 'max_height' ? (
              <NumberField
                name={`${dashPath}.video_quality.height`}
                label={<Trans>Maximum height (px)</Trans>}
                description={
                  <Trans>The smallest picture is used when none fits.</Trans>
                }
              />
            ) : type === 'closest_bandwidth' ? (
              <NumberField
                name={`${dashPath}.video_quality.bandwidth`}
                label={<Trans>Target bandwidth (bit/s)</Trans>}
              />
            ) : null
          }
        />
        <PolicyField
          label={<Trans>Audio Quality</Trans>}
          description={
            <Trans>
              Which audio version is recorded in the preferred language.
              Default: highest.
            </Trans>
          }
          path={`${dashPath}.audio_quality`}
          options={AUDIO_QUALITY_OPTIONS}
          renderPayload={(type) =>
            type === 'closest_bandwidth' ? (
              <NumberField
                name={`${dashPath}.audio_quality.bandwidth`}
                label={<Trans>Target bandwidth (bit/s)</Trans>}
              />
            ) : null
          }
        />
      </div>

      <FormField
        name={`${dashPath}.audio_language`}
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Preferred Audio Language</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Input
                {...field}
                value={field.value ?? ''}
                className={cn(CONFIG_INPUT, 'font-mono')}
                placeholder={i18n._(msg`e.g. en, pt-BR`)}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>
                A language tag. Any language is used when this is empty or no
                audio matches.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <div className={FIELD_GRID}>
        <TrackSwitch
          name={`${dashPath}.include_video`}
          label={<Trans>Record Video (Default: On)</Trans>}
          errorPath={`${dashPath}.include_audio`}
        />
        <TrackSwitch
          name={`${dashPath}.include_audio`}
          errorPath={`${dashPath}.include_audio`}
          label={<Trans>Record Separate Audio (Default: On)</Trans>}
          description={
            <Trans>Audio inside the video stream is always kept.</Trans>
          }
        />
      </div>
    </div>
  );
});
DashTrackSettings.displayName = 'DashTrackSettings';

const DashLiveStartSettings = React.memo(({ dashPath }: SectionProps) => (
  <PolicyField
    label={<Trans>Live Start Position</Trans>}
    description={
      <Trans>
        Where recording of a live stream starts. Automatic follows the delay
        suggested by the stream, else three segments behind the live edge.
        Finished streams are always recorded from the beginning.
      </Trans>
    }
    path={`${dashPath}.live_start`}
    options={LIVE_START_OPTIONS}
    renderPayload={(type) =>
      type === 'delay' ? (
        <NumberField
          name={`${dashPath}.live_start.delay_ms`}
          label={<Trans>Delay behind the live edge (ms)</Trans>}
        />
      ) : null
    }
  />
));
DashLiveStartSettings.displayName = 'DashLiveStartSettings';

const DashDownloadSettings = React.memo(({ dashPath }: SectionProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className={FIELD_GRID}>
      <NumberField
        name={`${dashPath}.download_concurrency`}
        label={<Trans>Download Concurrency</Trans>}
        description={<Trans>Segments downloaded at once, per track.</Trans>}
        placeholder={defaultPlaceholder(
          MESIO_DASH_DEFAULTS.download_concurrency,
        )}
      />
      <NumberField
        name={`${dashPath}.segment_timeout_ms`}
        label={<Trans>Segment Timeout (ms)</Trans>}
        placeholder={defaultPlaceholder(MESIO_DASH_DEFAULTS.segment_timeout_ms)}
      />
      <NumberField
        name={`${dashPath}.max_segment_retries`}
        label={<Trans>Segment Retries</Trans>}
        placeholder={defaultPlaceholder(
          MESIO_DASH_DEFAULTS.max_segment_retries,
        )}
      />
    </div>
  );
});
DashDownloadSettings.displayName = 'DashDownloadSettings';

const DashManifestSettings = React.memo(({ dashPath }: SectionProps) => {
  const defaultPlaceholder = useDefaultPlaceholder();
  return (
    <div className={FIELD_GRID}>
      <NumberField
        name={`${dashPath}.manifest_timeout_ms`}
        label={<Trans>Manifest Timeout (ms)</Trans>}
        placeholder={defaultPlaceholder(
          MESIO_DASH_DEFAULTS.manifest_timeout_ms,
        )}
      />
      <NumberField
        name={`${dashPath}.min_refresh_interval_ms`}
        label={<Trans>Minimum Refresh Interval (ms)</Trans>}
        description={
          <Trans>The manifest's own update period applies when longer.</Trans>
        }
        placeholder={defaultPlaceholder(
          MESIO_DASH_DEFAULTS.min_refresh_interval_ms,
        )}
      />
      <NumberField
        name={`${dashPath}.max_refresh_retries`}
        label={<Trans>Refresh Retries</Trans>}
        description={
          <Trans>
            Failed manifest refreshes in a row before the recording fails.
          </Trans>
        }
        placeholder={defaultPlaceholder(
          MESIO_DASH_DEFAULTS.max_refresh_retries,
        )}
      />
    </div>
  );
});
DashManifestSettings.displayName = 'DashManifestSettings';

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

interface MesioDashFormProps {
  basePath?: string;
}

/** MPEG-DASH settings of a mesio engine; empty fields use the engine defaults. */
export function MesioDashForm({ basePath = 'config' }: MesioDashFormProps) {
  const { i18n } = useLingui();
  const dashPath = `${basePath}.dash`;

  return (
    <Accordion type="multiple" defaultValue={['tracks']} className="w-full">
      {DASH_SECTIONS.map(({ value, icon: Icon, label, Section }) => (
        <AccordionItem key={value} value={value} className="border-border/50">
          <AccordionTrigger className="py-3 hover:no-underline">
            <span className="flex items-center gap-2.5">
              <Icon className="h-4 w-4 text-muted-foreground" />
              <span className="text-sm font-medium">{i18n._(label)}</span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="pb-4 pt-1">
            <Section dashPath={dashPath} />
          </AccordionContent>
        </AccordionItem>
      ))}
    </Accordion>
  );
}
