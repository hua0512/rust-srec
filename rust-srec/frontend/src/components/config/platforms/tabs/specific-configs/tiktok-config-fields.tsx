import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Zap } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
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
                <Select
                  onValueChange={field.onChange}
                  value={field.value || 'auto'}
                >
                  <FormControl>
                    <SelectTrigger className={CONFIG_INPUT}>
                      <SelectValue placeholder={i18n._(msg`Select API Mode`)} />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent className={CONFIG_SELECT_CONTENT}>
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

          <FlagFormField
            fieldName={configPath<TFieldValues>(
              fieldName,
              'force_origin_quality',
            )}
            title={<Trans>Force Origin Quality</Trans>}
            description={
              <Trans>
                Only keep the original-quality stream when TikTok offers one;
                other qualities are ignored.
              </Trans>
            }
            checked={(value) => !!value}
          />
        </div>
      </section>
    </div>
  );
}
