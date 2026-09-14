import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Zap } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface TikTokConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

export function TikTokConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: TikTokConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();

  return (
    <div className="space-y-12">
      {/* Extraction Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Zap} accent="indigo">
          <Trans>Extraction Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'api_mode')}
            render={({ field }) => (
              <FormItem>
                <ConfigFieldLabel accent="indigo" className="mb-3">
                  <Trans>Extraction API Mode</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value || 'auto'}
                  >
                    <SelectTrigger className="bg-background/50 h-11 rounded-xl border-border/50 focus:bg-background transition-all shadow-sm">
                      <SelectValue placeholder={i18n._(msg`Select API Mode`)} />
                    </SelectTrigger>
                    <SelectContent className="rounded-xl border-border/50 shadow-xl">
                      <SelectItem value="auto">
                        <Trans>Auto</Trans>{' '}
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Default</Trans>)
                        </span>
                      </SelectItem>
                      <SelectItem value="web">
                        <Trans>Web API</Trans>
                      </SelectItem>
                      <SelectItem value="html">
                        <Trans>Live page HTML</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    Auto tries the signature-free Web API first and falls back
                    to parsing the live page. The live page is more likely to
                    hit bot checks.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'force_origin_quality')}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-2xl border bg-muted/5 p-5 transition-all hover:bg-muted/10 border-border/50">
                <div className="space-y-1.5 pr-4">
                  <FormLabel className="text-sm font-bold text-foreground">
                    <Trans>Force Origin Quality</Trans>
                  </FormLabel>
                  <FormDescription className="text-xs leading-relaxed font-medium">
                    <Trans>
                      Only keep the original-quality stream when TikTok offers
                      one; other qualities are ignored.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>
      </section>
    </div>
  );
}
