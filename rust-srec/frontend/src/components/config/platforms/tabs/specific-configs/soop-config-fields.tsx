import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Lock } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
  CONFIG_DESCRIPTION,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface SoopConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

export function SoopConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: SoopConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={Lock} accent="emerald">
          <Trans>Stream Password</Trans>
        </ConfigSectionHeading>

        <FormField
          control={form.control}
          name={configPath<TFieldValues>(fieldName, 'stream_password')}
          render={({ field }) => (
            <FormItem className="space-y-4 max-w-xl">
              <ConfigFieldLabel accent="emerald">
                <Trans>Stream Password</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  type="password"
                  autoComplete="off"
                  {...field}
                  value={field.value || ''}
                  className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                  placeholder={i18n._(msg`Password...`)}
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>
                  Default password for password-protected rooms (can be
                  overridden per-streamer with ?pwd= in the URL).
                </Trans>
              </FormDescription>
            </FormItem>
          )}
        />
      </section>
    </div>
  );
}
