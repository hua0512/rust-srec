import { UseFormReturn } from 'react-hook-form';
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
import { useDefaultPlaceholder } from '@/hooks/use-default-placeholder';
import { NumberInput } from '@/components/ui/number-input';

interface DouyuConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
  inherited?: boolean;
}

export function DouyuConfigFields({
  form,
  fieldName,
  inherited = false,
}: DouyuConfigFieldsProps) {
  const { i18n } = useLingui();
  const defaultPlaceholder = useDefaultPlaceholder();
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
              name={`${fieldName}.cdn`}
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
                      placeholder={unsetPlaceholder('ws-h5')}
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
                      Specify preferred content delivery network (e.g., ws-h5,
                      hw-h5).
                    </Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name={`${fieldName}.rate`}
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

      {/* Network & Content Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Gamepad2} accent="indigo">
          <Trans>Network & Content</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.disable_interactive_game`}
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
            name={`${fieldName}.request_retries`}
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
                    Max number of retry attempts for metadata fetching.
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
