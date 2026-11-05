import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';
import { memo, type ReactNode } from 'react';
import {
  useWatch,
  useFormContext,
  type FieldPath,
  type FieldValues,
} from 'react-hook-form';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { SETTING_ROW_CLASS } from '@/components/ui/switch-card';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { Tabs, TabsContent, TabsList } from '@/components/ui/tabs';
import {
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorListField,
  ProcessorNumberField,
  ProcessorSection,
  ProcessorSelectField,
  ProcessorTabTrigger,
  ProcessorTextField,
} from './processor-form-layout';
import {
  RemuxConfigSchema,
  SOFTWARE_PRESETS,
  type PresetOption,
} from '../processor-schemas';
import { z } from 'zod';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import {
  Settings2,
  Film,
  Music,
  Sliders,
  Box,
  Layers,
  Play,
  Square,
  Timer,
  MousePointer2,
  Plus,
  Trash2,
  Tags,
  type LucideIcon,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import { useRowKeys } from '@/hooks/use-row-keys';

type RemuxConfig = z.infer<typeof RemuxConfigSchema>;

const NVENC_CODECS = ['h264nvenc', 'hevcnvenc', 'av1nvenc'] as const;

type CodecFamily = 'software' | 'nvenc' | 'av1nvenc';

function getCodecFamily(codec: string | undefined): CodecFamily {
  if (codec === 'av1nvenc') return 'av1nvenc';
  if (NVENC_CODECS.includes(codec as (typeof NVENC_CODECS)[number]))
    return 'nvenc';
  return 'software';
}

const COMMON_NVENC_PRESETS: PresetOption[] = [
  { value: 'fast', label: 'Fast' },
  { value: 'medium', label: 'Medium' },
  { value: 'slow', label: 'Slow' },
];

const P_PRESETS: PresetOption[] = [
  { value: 'p1', label: 'P1', hint: msg`Fastest` },
  { value: 'p2', label: 'P2' },
  { value: 'p3', label: 'P3' },
  { value: 'p4', label: 'P4', hint: msg`Default` },
  { value: 'p5', label: 'P5' },
  { value: 'p6', label: 'P6' },
  { value: 'p7', label: 'P7', hint: msg`Best quality` },
];

const NVENC_LEGACY_PRESETS: PresetOption[] = [
  { value: 'hp', label: 'HP', hint: msg`High performance` },
  { value: 'hq', label: 'HQ', hint: msg`High quality` },
  { value: 'bd', label: 'BD', hint: msg`Blu-ray Disc` },
  { value: 'll', label: 'LL', hint: msg`Low latency` },
  { value: 'llhq', label: 'LLHQ', hint: msg`Low latency, high quality` },
  { value: 'llhp', label: 'LLHP', hint: msg`Low latency, high performance` },
  { value: 'lossless', label: 'Lossless' },
  { value: 'losslesshp', label: 'Lossless HP' },
];

function getPresetsForCodecFamily(family: CodecFamily) {
  switch (family) {
    case 'av1nvenc':
      return [...COMMON_NVENC_PRESETS, ...P_PRESETS];
    case 'nvenc':
      return [...COMMON_NVENC_PRESETS, ...NVENC_LEGACY_PRESETS, ...P_PRESETS];
    case 'software':
      return SOFTWARE_PRESETS;
  }
}

function MetadataTagRows({
  value,
  onChange,
}: {
  value: Array<[string, string]>;
  onChange: (next: Array<[string, string]>) => void;
}) {
  const { i18n } = useLingui();
  const rowKeys = useRowKeys(value.length);

  if (value.length === 0) {
    return (
      <div className="space-y-2">
        <div className="text-[11px] text-muted-foreground/50 text-center py-4 border border-dashed border-border/30 rounded-lg">
          <Trans>No metadata tags added</Trans>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {value.map(([key, tagValue], index) => (
        <div
          key={rowKeys.keyAt(index)}
          className="flex gap-2 items-center group animate-in fade-in slide-in-from-top-1 duration-200"
        >
          <div className="relative flex-1 group/input">
            <Tags className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground/40 group-focus-within/input:text-primary/50 transition-colors" />
            <Input
              placeholder={i18n._(msg`Key`)}
              aria-label={i18n._(msg`Key`)}
              value={key}
              onChange={(e) =>
                onChange(
                  value.map((pair, i) =>
                    i === index ? [e.target.value, tagValue] : pair,
                  ),
                )
              }
              className="h-9 pl-8 bg-background/50 border-border/50 focus:bg-background text-xs"
            />
          </div>
          <Input
            placeholder={i18n._(msg`Value`)}
            aria-label={i18n._(msg`Value`)}
            value={tagValue}
            onChange={(e) =>
              onChange(
                value.map((pair, i) =>
                  i === index ? [key, e.target.value] : pair,
                ),
              )
            }
            className="flex-1 h-9 bg-background/50 border-border/50 focus:bg-background text-xs"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-9 w-9 shrink-0 text-muted-foreground/30 hover:text-destructive hover:bg-destructive/10"
            onClick={() => {
              rowKeys.removeAt(index);
              onChange(value.filter((_, i) => i !== index));
            }}
            aria-label={i18n._(msg`Remove custom tag`)}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
    </div>
  );
}

const REMUX_TABS = [
  { value: 'general', icon: Settings2, label: <Trans>General</Trans> },
  { value: 'format', icon: Box, label: <Trans>Format</Trans> },
  { value: 'video', icon: Film, label: <Trans>Video</Trans> },
  { value: 'audio', icon: Music, label: <Trans>Audio</Trans> },
  { value: 'advanced', icon: Sliders, label: <Trans>Advanced</Trans> },
];

const AUDIO_CODECS = [
  { value: 'copy', label: <Trans>Copy (Passthrough)</Trans> },
  { value: 'aac', label: 'AAC' },
  { value: 'mp3', label: 'MP3' },
  { value: 'opus', label: 'Opus' },
  { value: 'flac', label: 'FLAC' },
  { value: 'none', label: <Trans>None (Strip Audio)</Trans> },
];

/** Start, duration and end of the trimmed range; clearing one stores 0. */
function TrimTimeField({
  control,
  name,
  icon,
  label,
}: {
  control: ProcessorConfigFormProps['control'];
  name: FieldPath<FieldValues>;
  icon: LucideIcon;
  label: ReactNode;
}) {
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem>
          <ConfigFieldLabel icon={icon}>{label}</ConfigFieldLabel>
          <FormControl>
            <InputWithUnit
              unitType="duration"
              value={field.value}
              onChange={(val: number | null) => field.onChange(val ?? 0)}
              className="bg-background/50"
            />
          </FormControl>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

export const RemuxConfigForm = memo(function RemuxConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<RemuxConfig>(pathPrefix);
  const { setValue } = useFormContext();

  const videoCodec = useWatch({
    control,
    name: fieldPath('video_codec'),
  });

  const audioCodec = useWatch({
    control,
    name: fieldPath('audio_codec'),
  });

  const format = useWatch({
    control,
    name: fieldPath('format'),
  });

  const hwaccel = useWatch({
    control,
    name: fieldPath('hwaccel'),
  });

  const isVideoCopy = !videoCodec || videoCodec === 'copy';
  const isAudioCopy = !audioCodec || audioCodec === 'copy';
  const codecFamily = getCodecFamily(videoCodec);
  const isNvenc = codecFamily !== 'software';
  const isCuda = hwaccel === 'cuda';
  const cqMax = codecFamily === 'av1nvenc' ? 63 : 51;
  const presetOptions = getPresetsForCodecFamily(codecFamily);

  // The backend adds `-movflags +faststart` only for these containers, so for any other format
  // the switch shows off and is locked while the stored choice is kept for a later switch back.
  const faststartSupported =
    !format || ['mp4', 'mov', 'm4v'].includes(format.toLowerCase());

  // Runs only when the user picks a codec. Resetting the preset from an
  // effect instead also fired when an editor loaded a saved config into an
  // empty form, replacing a stored NVENC preset with 'medium'.
  const handleVideoCodecChange = (newCodec: string) => {
    const family = getCodecFamily(newCodec);
    if (family !== codecFamily) {
      // Presets differ per codec family; drop one the new family may reject.
      setValue(fieldPath('preset'), 'medium');
    }
    if (family !== 'software' && hwaccel !== 'cuda') {
      setValue(fieldPath('hwaccel'), 'cuda');
    }
    return newCodec;
  };

  // Handle hwaccel change: reset NVENC codec if cuda is deselected
  const handleHwaccelChange = (newHwaccel: string) => {
    if (newHwaccel !== 'cuda' && isNvenc) {
      setValue(fieldPath('video_codec'), 'copy');
      setValue(fieldPath('preset'), 'medium');
    }
    return newHwaccel;
  };

  return (
    <ProcessorFormShell>
      <Tabs defaultValue="general" className="w-full">
        <TabsList className="w-full h-auto p-1.5 bg-muted/20 rounded-xl flex flex-wrap gap-1.5 backdrop-blur-sm mb-6">
          {REMUX_TABS.map((tab) => (
            <ProcessorTabTrigger
              key={tab.value}
              value={tab.value}
              icon={tab.icon}
            >
              {tab.label}
            </ProcessorTabTrigger>
          ))}
        </TabsList>

        <div className="space-y-4">
          <TabsContent
            value="general"
            className="space-y-4 focus-visible:outline-none focus-visible:ring-0"
          >
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
              <ProcessorSection
                icon={Film}
                iconClassName="text-purple-500"
                title={<Trans>Video Settings</Trans>}
              >
                <FormField
                  control={control}
                  name={fieldPath('video_codec')}
                  render={({ field }) => (
                    <FormItem>
                      <ConfigFieldLabel>
                        <Trans>Video Codec</Trans>
                      </ConfigFieldLabel>
                      <Select
                        onValueChange={(v) => {
                          // Radix can emit '' while a reset mounts the
                          // options (see editor/preset-meta-form.tsx).
                          if (!v) return;
                          handleVideoCodecChange(v);
                          field.onChange(v);
                        }}
                        value={field.value ?? ''}
                      >
                        <FormControl>
                          <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                            <SelectValue
                              placeholder={i18n._(msg`Select codec`)}
                            />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent className={CONFIG_SELECT_CONTENT}>
                          <SelectItem value="copy">
                            <Trans>Copy (Passthrough)</Trans>
                          </SelectItem>
                          <SelectItem value="h264">H.264</SelectItem>
                          <SelectItem value="h265">H.265 / HEVC</SelectItem>
                          <SelectItem value="vp9">VP9</SelectItem>
                          <SelectItem value="av1">AV1</SelectItem>
                          {isCuda && (
                            <>
                              <SelectItem value="h264nvenc">
                                H.264 NVENC (NVIDIA)
                              </SelectItem>
                              <SelectItem value="hevcnvenc">
                                H.265 NVENC (NVIDIA)
                              </SelectItem>
                              <SelectItem value="av1nvenc">
                                AV1 NVENC (NVIDIA RTX 4000+)
                              </SelectItem>
                            </>
                          )}
                        </SelectContent>
                      </Select>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                {!isVideoCopy && (
                  <FormField
                    control={control}
                    name={fieldPath('preset')}
                    render={({ field }) => (
                      <FormItem>
                        <div className="flex justify-between items-center">
                          <ConfigFieldLabel>
                            <Trans>Encoding Preset</Trans>
                          </ConfigFieldLabel>
                          <span className="text-[10px] text-muted-foreground/70 uppercase tracking-wider">
                            {field.value || 'medium'}
                          </span>
                        </div>
                        <Select
                          onValueChange={field.onChange}
                          value={field.value || 'medium'}
                        >
                          <FormControl>
                            <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                              <SelectValue
                                placeholder={i18n._(msg`Select preset`)}
                              />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent className={CONFIG_SELECT_CONTENT}>
                            {presetOptions.map((p) => (
                              <SelectItem key={p.value} value={p.value}>
                                {p.hint
                                  ? `${p.label} (${i18n._(p.hint)})`
                                  : p.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                )}
              </ProcessorSection>

              <div className="space-y-4">
                <ProcessorSection
                  icon={Music}
                  iconClassName="text-pink-500"
                  title={<Trans>Audio Settings</Trans>}
                >
                  <ProcessorSelectField
                    control={control}
                    name={fieldPath('audio_codec')}
                    label={<Trans>Audio Codec</Trans>}
                    placeholder={i18n._(msg`Select codec`)}
                    options={AUDIO_CODECS}
                  />
                </ProcessorSection>

                <ProcessorSection
                  icon={Box}
                  iconClassName="text-blue-500"
                  title={<Trans>Container</Trans>}
                >
                  <ProcessorTextField
                    control={control}
                    name={fieldPath('format')}
                    label={<Trans>Output Format</Trans>}
                    placeholder="mp4"
                  />
                </ProcessorSection>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="format"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            {!isVideoCopy && (
              <ProcessorSection
                icon={Layers}
                iconClassName="text-emerald-500"
                title={<Trans>Quality Control</Trans>}
              >
                <FormField
                  control={control}
                  name={fieldPath('crf')}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex justify-between items-end mb-2">
                        <ConfigFieldLabel>
                          {isNvenc ? (
                            <Trans>CQ (Constant Quality)</Trans>
                          ) : (
                            <Trans>CRF (Constant Rate Factor)</Trans>
                          )}
                        </ConfigFieldLabel>
                        <span className="font-mono text-sm font-medium bg-background px-2 py-0.5 rounded border border-border/50">
                          {field.value ?? 23}
                        </span>
                      </div>
                      <div className="flex items-center gap-4">
                        <span
                          aria-hidden="true"
                          className="text-[10px] text-muted-foreground font-medium w-8 text-right"
                        >
                          <Trans context="quality">Best</Trans>
                        </span>
                        <FormControl>
                          <Slider
                            aria-label={
                              isNvenc
                                ? i18n._(msg`CQ (Constant Quality)`)
                                : i18n._(msg`CRF (Constant Rate Factor)`)
                            }
                            min={0}
                            max={cqMax}
                            step={1}
                            value={[field.value ?? 23]}
                            onValueChange={(val: number[]) =>
                              field.onChange(val[0])
                            }
                            className="flex-1"
                          />
                        </FormControl>
                        <span
                          aria-hidden="true"
                          className="text-[10px] text-muted-foreground font-medium w-8"
                        >
                          <Trans context="quality">Worst</Trans>
                        </span>
                      </div>
                      <FormDescription className="text-[11px] text-right pt-1">
                        <Trans>Lower value = Higher Quality</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </ProcessorSection>
            )}

            {(!isVideoCopy || !isAudioCopy) && (
              <ProcessorSection
                icon={Sliders}
                iconClassName="text-cyan-500"
                title={<Trans>Target Bitrate</Trans>}
              >
                <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                  {!isVideoCopy && (
                    <ProcessorTextField
                      control={control}
                      name={fieldPath('video_bitrate')}
                      label={<Trans>Video Bitrate</Trans>}
                      placeholder={i18n._(msg`e.g. 5000k`)}
                      className="font-mono text-sm"
                    />
                  )}
                  {!isAudioCopy && (
                    <ProcessorTextField
                      control={control}
                      name={fieldPath('audio_bitrate')}
                      label={<Trans>Audio Bitrate</Trans>}
                      placeholder={i18n._(msg`e.g. 192k`)}
                      className="font-mono text-sm"
                    />
                  )}
                </div>
              </ProcessorSection>
            )}
          </TabsContent>

          <TabsContent
            value="video"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            {!isVideoCopy && (
              <ProcessorSection
                icon={Box}
                iconClassName="text-orange-500"
                title={<Trans>Dimensions & Filters</Trans>}
              >
                <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                  <ProcessorTextField
                    control={control}
                    name={fieldPath('resolution')}
                    label={<Trans>Resolution (Scale)</Trans>}
                    placeholder={i18n._(msg`e.g. 1920:1080`)}
                    className="font-mono text-sm"
                  />
                  <ProcessorNumberField
                    control={control}
                    name={fieldPath('fps')}
                    label={<Trans>Framerate (FPS)</Trans>}
                    placeholder={i18n._(msg`e.g. 60`)}
                    className="font-mono text-sm"
                  />
                  <div className="md:col-span-2">
                    <ProcessorTextField
                      control={control}
                      name={fieldPath('video_filter')}
                      label={<Trans>FFmpeg Video Filter</Trans>}
                      placeholder={i18n._(
                        msg`e.g. hflip,noise=alls=20:allf=t+u`,
                      )}
                      className="font-mono text-xs"
                    />
                  </div>
                </div>
              </ProcessorSection>
            )}

            <ProcessorSection
              icon={Timer}
              iconClassName="text-indigo-500"
              title={<Trans>Trimming & Time</Trans>}
            >
              <div className="flex flex-col md:flex-row gap-4 items-end">
                <div className="flex-1 w-full">
                  <TrimTimeField
                    control={control}
                    name={fieldPath('start_time')}
                    icon={Play}
                    label={<Trans>Start Time</Trans>}
                  />
                </div>
                <div className="hidden md:flex h-11 items-center pb-2 text-muted-foreground/30">
                  <span className="text-xl">→</span>
                </div>
                <div className="flex-1 w-full">
                  <TrimTimeField
                    control={control}
                    name={fieldPath('duration')}
                    icon={Timer}
                    label={<Trans>Duration</Trans>}
                  />
                </div>
                <div className="hidden md:flex h-11 items-center pb-2 text-muted-foreground/30">
                  <span className="text-xl">→</span>
                </div>
                <div className="flex-1 w-full">
                  <TrimTimeField
                    control={control}
                    name={fieldPath('end_time')}
                    icon={Square}
                    label={<Trans>End Time</Trans>}
                  />
                </div>
              </div>
            </ProcessorSection>
          </TabsContent>

          <TabsContent
            value="audio"
            className="space-y-4 focus-visible:outline-none focus-visible:ring-0"
          >
            {!isAudioCopy && (
              <ProcessorSection
                icon={MousePointer2}
                iconClassName="text-pink-500"
                title={<Trans>Audio Processing</Trans>}
              >
                <ProcessorTextField
                  control={control}
                  name={fieldPath('audio_filter')}
                  label={<Trans>FFmpeg Audio Filter (-af)</Trans>}
                  placeholder={i18n._(msg`e.g. volume=0.5`)}
                  className="font-mono text-xs"
                />
              </ProcessorSection>
            )}
          </TabsContent>

          <TabsContent
            value="advanced"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
              <div className="space-y-4">
                <ProcessorSection
                  icon={Settings2}
                  iconClassName="text-gray-500"
                  title={<Trans>Performance</Trans>}
                >
                  <FormField
                    control={control}
                    name={fieldPath('hwaccel')}
                    render={({ field }) => (
                      <FormItem>
                        <ConfigFieldLabel>
                          <Trans>Hardware Acceleration</Trans>
                        </ConfigFieldLabel>
                        <Select
                          onValueChange={(v) => {
                            handleHwaccelChange(v);
                            field.onChange(v);
                          }}
                          value={field.value ?? ''}
                        >
                          <FormControl>
                            <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                              <SelectValue placeholder={i18n._(msg`None`)} />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent className={CONFIG_SELECT_CONTENT}>
                            <SelectItem value="none">None</SelectItem>
                            <SelectItem value="cuda">CUDA (NVIDIA)</SelectItem>
                            <SelectItem value="vaapi">
                              VAAPI (Intel/AMD)
                            </SelectItem>
                            <SelectItem value="qsv">QSV (Intel)</SelectItem>
                            <SelectItem value="videotoolbox">
                              VideoToolbox (Mac)
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </ProcessorSection>
                <div className="space-y-3">
                  <FormField
                    control={control}
                    name={fieldPath('faststart')}
                    render={({ field }) => (
                      <FormItem
                        className={cn(
                          SETTING_ROW_CLASS,
                          !faststartSupported && 'opacity-50',
                        )}
                      >
                        <div className="min-w-0 space-y-1">
                          <FormLabel>
                            <Trans>Web Optimize</Trans>
                          </FormLabel>
                          <FormDescription className="text-xs leading-relaxed">
                            <Trans>
                              Use <code>-movflags +faststart</code>
                            </Trans>
                            {!faststartSupported && (
                              <span className="block text-destructive/70 mt-1">
                                <Trans>Only supported for MP4/MOV/M4V</Trans>
                              </span>
                            )}
                          </FormDescription>
                        </div>
                        <FormControl>
                          <Switch
                            className="shrink-0"
                            checked={faststartSupported && field.value}
                            onCheckedChange={field.onChange}
                            disabled={!faststartSupported}
                          />
                        </FormControl>
                      </FormItem>
                    )}
                  />
                  <FlagFormField
                    fieldName={fieldPath('overwrite')}
                    title={<Trans>Overwrite Files</Trans>}
                    description={
                      <Trans>Force overwrite if output exists</Trans>
                    }
                  />
                  <FlagFormField
                    fieldName={fieldPath('remove_input_on_success')}
                    title={<Trans>Remove Input on Success</Trans>}
                    description={
                      <Trans>Delete original file after successful remux</Trans>
                    }
                  />
                </div>
              </div>

              <ProcessorSection
                icon={Sliders}
                iconClassName="text-amber-500"
                title={<Trans>Custom Flags</Trans>}
              >
                <ProcessorListField
                  control={control}
                  name={fieldPath('input_options')}
                  label={<Trans>Input Options (-i flags)</Trans>}
                  placeholder={i18n._(msg`--flag value`)}
                />
                <ProcessorListField
                  control={control}
                  name={fieldPath('output_options')}
                  label={<Trans>Output Options</Trans>}
                  description={<Trans>Appended before output filename</Trans>}
                  placeholder={i18n._(msg`--flag value`)}
                />
              </ProcessorSection>

              <ProcessorSection
                icon={Layers}
                iconClassName="text-blue-500"
                title={<Trans>Streams & Metadata</Trans>}
              >
                <ProcessorListField
                  control={control}
                  name={fieldPath('map_streams')}
                  label={<Trans>Stream Mapping (-map)</Trans>}
                  description={<Trans>e.g. 0:v:0, 0:a:0</Trans>}
                  placeholder="0:v:0"
                />
                <FlagFormField
                  fieldName={fieldPath('include_recording_metadata')}
                  title={<Trans>Include Recording Metadata</Trans>}
                  description={
                    <Trans>
                      Add the recorded title, streamer, and UTC recording date
                      during remux. Tags below override these values.
                    </Trans>
                  }
                />
                <FormField
                  control={control}
                  name={fieldPath('metadata')}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center justify-between mb-2">
                        <ConfigFieldLabel>
                          <Trans>Metadata Tags</Trans>
                        </ConfigFieldLabel>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="h-7 px-2 text-[10px] text-primary hover:text-primary hover:bg-primary/10"
                          onClick={() => {
                            const current = field.value || [];
                            field.onChange([...current, ['', '']]);
                          }}
                        >
                          <Plus className="w-3 h-3 mr-1" />
                          <Trans>Add Tag</Trans>
                        </Button>
                      </div>
                      <MetadataTagRows
                        value={field.value || []}
                        onChange={field.onChange}
                      />
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </ProcessorSection>
            </div>
          </TabsContent>
        </div>
      </Tabs>
    </ProcessorFormShell>
  );
});
