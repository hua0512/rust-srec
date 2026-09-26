import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Switch } from '@/components/ui/switch';
import { Zap, Cloud, Gamepad2, RotateCcw } from 'lucide-react';
import { DouyuQualityCombobox } from './douyu-quality-combobox';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';
import { useDefaultPlaceholder } from '@/hooks/use-default-placeholder';
import { NumberInput } from '@/components/ui/number-input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

interface DouyuConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
  inherited?: boolean;
}

export function DouyuConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
  inherited = false,
}: DouyuConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();
  const defaultPlaceholder = useDefaultPlaceholder();
  const apiMode = useWatch({
    control: form.control,
    name: configPath<TFieldValues>(fieldName, 'api_mode'),
  });
  const onlyAudio = useWatch({
    control: form.control,
    name: configPath<TFieldValues>(fieldName, 'only_audio'),
  });
  // A template or streamer layer leaves a blank field to the layer above it;
  // the platform layer falls back to the extractor's own default instead.
  const unsetPlaceholder = (fallback: string | number) =>
    inherited ? i18n._(msg`Inherited`) : defaultPlaceholder(fallback);

  return (
    <div className="space-y-12">
      {/* Extraction Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Zap} accent="indigo">
          <Trans>Extraction Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={form.control}
              name={configPath<TFieldValues>(fieldName, 'api_mode')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Extraction Method</Trans>
                  </FormLabel>
                  <Select
                    value={field.value ?? 'unset'}
                    onValueChange={(value) =>
                      field.onChange(value === 'unset' ? null : value)
                    }
                  >
                    <FormControl>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="unset">
                        {unsetPlaceholder('App')}
                      </SelectItem>
                      <SelectItem value="app">
                        <Trans>Android App</Trans>
                      </SelectItem>
                      <SelectItem value="web">
                        <Trans>Web (deprecated)</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription>
                    <Trans>
                      App uses anonymous playback. Web is retained for
                      compatibility. Audio only always uses Web.
                    </Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name={configPath<TFieldValues>(fieldName, 'codec')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Preferred Video Codec</Trans>
                  </FormLabel>
                  <Select
                    value={field.value ?? 'unset'}
                    onValueChange={(value) =>
                      field.onChange(value === 'unset' ? null : value)
                    }
                  >
                    <FormControl>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="unset">
                        {unsetPlaceholder('AVC')}
                      </SelectItem>
                      <SelectItem value="avc">AVC (H.264)</SelectItem>
                      <SelectItem value="hevc">HEVC (H.265)</SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription>
                    <Trans>
                      Prefer HEVC when available, with AVC fallback. Ignored for
                      audio only.
                    </Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={form.control}
              name={configPath<TFieldValues>(fieldName, 'cdn')}
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-center gap-2 mb-3">
                    <Cloud className="w-4 h-4 text-muted-foreground" />
                    <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                      <Trans>Preferred CDN</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Input
                      placeholder={unsetPlaceholder(
                        apiMode === 'web' || onlyAudio ? 'ws-h5' : 'hw',
                      )}
                      {...field}
                      value={field.value ?? ''}
                      // Null rather than an empty string, which the extractor
                      // would read as a CDN preference of its own.
                      onChange={(e) => field.onChange(e.target.value || null)}
                      className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all"
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] font-medium pt-1 px-1">
                    <Trans>
                      App CDN examples: hw, tct, hs, ws. Legacy -h5 suffixes are
                      accepted.
                    </Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name={configPath<TFieldValues>(fieldName, 'rate')}
              render={({ field }) => (
                <FormItem>
                  <ConfigFieldLabel accent="indigo" className="mb-3">
                    <Trans>Quality Rate</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <DouyuQualityCombobox
                      fieldName={fieldName}
                      form={form}
                      value={field.value}
                      onChange={field.onChange}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] font-medium pt-1">
                    <Trans>
                      Select a preset or type a Douyu rate. Audio only requests
                      AAC without video.
                    </Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
          </div>
        </div>
      </section>

      <section className="space-y-6">
        <ConfigSectionHeading icon={Zap} accent="indigo">
          <Trans>App Device</Trans>
        </ConfigSectionHeading>
        <FormDescription>
          <Trans>
            App requests share one device identity across retries. The user
            agent follows the model and Android version.
          </Trans>
        </FormDescription>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          {[
            {
              key: 'device_name',
              label: msg`Device Model`,
              fallback: i18n._(msg`Random model`),
            },
            { key: 'os_version', label: msg`Android Version`, fallback: '14' },
            {
              key: 'device_id',
              label: msg`Device ID`,
              fallback: i18n._(msg`Automatic`),
            },
          ].map(({ key, label, fallback }) => (
            <FormField
              key={key}
              control={form.control}
              name={configPath<TFieldValues>(fieldName, key)}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>{i18n._(label)}</FormLabel>
                  <FormControl>
                    <Input
                      {...field}
                      value={field.value ?? ''}
                      placeholder={unsetPlaceholder(fallback)}
                      onChange={(e) => field.onChange(e.target.value || null)}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          ))}
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'device_id_mode')}
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Device ID source</Trans>
                </FormLabel>
                <Select
                  value={field.value ?? 'unset'}
                  onValueChange={(value) =>
                    field.onChange(value === 'unset' ? null : value)
                  }
                >
                  <FormControl>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="unset">
                      {unsetPlaceholder(i18n._(msg`Local generation`))}
                    </SelectItem>
                    <SelectItem value="local">
                      <Trans>Local generation</Trans>
                    </SelectItem>
                    <SelectItem value="server">
                      <Trans>Server registration</Trans>
                    </SelectItem>
                    <SelectItem value="default">
                      <Trans>Fixed fallback</Trans>
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormItem>
            )}
          />
        </div>
        <FormDescription>
          <Trans>
            An explicit Device ID overrides the acf_did cookie and selected
            source. Server registration requires an additional request and is
            reused by this extractor.
          </Trans>
        </FormDescription>
      </section>

      {/* Network & Content Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Gamepad2} accent="indigo">
          <Trans>Network & Content</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(
              fieldName,
              'disable_interactive_game',
            )}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                <div className="space-y-0.5">
                  <FormLabel className="text-xs font-bold text-foreground">
                    <Trans>Filter Interactive Games</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px] leading-tight font-medium text-muted-foreground/80">
                    <Trans>
                      Treat interactive games as offline platforms during
                      extraction checks.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                    className="scale-90"
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'request_retries')}
            render={({ field }) => (
              <FormItem>
                <div className="flex items-center gap-2 mb-3">
                  <RotateCcw className="w-4 h-4 text-muted-foreground" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>API Request Retries</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <NumberInput
                    field={field}
                    min={0}
                    step={1}
                    placeholder={unsetPlaceholder(3)}
                    // Null rather than an absent key: the config resolver only
                    // lets a stated null override a legacy flat template value.
                    onChange={(value) => field.onChange(value ?? null)}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all max-w-[120px]"
                  />
                </FormControl>
                <FormDescription className="text-[10px] font-medium pt-1">
                  <Trans>
                    Maximum attempts for metadata and App playback requests.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>
    </div>
  );
}
