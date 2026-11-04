import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
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
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import { ProcessorFormShell, ProcessorSection } from './processor-form-layout';
import { AssBurninConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Video, Type, Settings, Trash2 } from 'lucide-react';
import { NumberInput } from '@/components/ui/number-input';

type AssBurninConfig = z.infer<typeof AssBurninConfigSchema>;

export function AssBurninConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<AssBurninConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        {/* Encoding Settings */}
        <ProcessorSection
          icon={Video}
          iconClassName="text-blue-500"
          title={<Trans>Encoding Settings</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('video_codec')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Video Codec</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      placeholder={i18n._(msg`libx264`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('audio_codec')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Audio Codec</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      placeholder={i18n._(msg`copy`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('crf')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>CRF (0-51)</Trans>
                  </FormLabel>
                  <FormControl>
                    <NumberInput
                      field={field}
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      min={0}
                      max={51}
                      step={1}
                      placeholder="23"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('preset')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Encoder Preset</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value ?? ''}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:ring-primary/20 rounded-lg">
                        <SelectValue placeholder={i18n._(msg`veryfast`)} />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="ultrafast">ultrafast</SelectItem>
                      <SelectItem value="superfast">superfast</SelectItem>
                      <SelectItem value="veryfast">veryfast</SelectItem>
                      <SelectItem value="faster">faster</SelectItem>
                      <SelectItem value="fast">fast</SelectItem>
                      <SelectItem value="medium">medium</SelectItem>
                      <SelectItem value="slow">slow</SelectItem>
                      <SelectItem value="slower">slower</SelectItem>
                      <SelectItem value="veryslow">veryslow</SelectItem>
                    </SelectContent>
                  </Select>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </ProcessorSection>

        {/* Subtitle Matching */}
        <ProcessorSection
          icon={Type}
          iconClassName="text-indigo-500"
          title={<Trans>Subtitle Matching</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('match_strategy')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Match Strategy</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value ?? ''}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:ring-primary/20 rounded-lg">
                        <SelectValue
                          placeholder={i18n._(msg`Select strategy`)}
                        />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="manifest">
                        <Trans>Session pairing (same segment)</Trans>
                      </SelectItem>
                      <SelectItem value="stem">
                        <Trans>Stem (Filename Match)</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>How to pair video files with .ass subtitles</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('require_ass')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 self-end h-11">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Require Subtitles</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>
        </ProcessorSection>

        {/* Advanced Settings */}
        <ProcessorSection
          icon={Settings}
          iconClassName="text-slate-500"
          title={<Trans>Advanced Settings</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('ffmpeg_path')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>FFmpeg Path</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      placeholder={i18n._(msg`ffmpeg`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('fonts_dir')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Fonts Directory</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      placeholder={i18n._(msg`/usr/share/fonts`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('passthrough_inputs')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Passthrough Inputs</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('exclude_ass_from_passthrough')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Exclude ASS from Passthrough</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>
        </ProcessorSection>

        {/* Cleanup Settings */}
        <ProcessorSection
          icon={Trash2}
          iconClassName="text-red-500"
          title={<Trans>Cleanup</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('delete_source_videos_on_success')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 border-red-500/20">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs text-red-600 dark:text-red-400">
                      <Trans>Delete Source Videos</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('delete_source_ass_on_success')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 border-red-500/20">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs text-red-600 dark:text-red-400">
                      <Trans>Delete Source ASS</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
