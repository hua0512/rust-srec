import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Lock, Shield } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
  CONFIG_DESCRIPTION,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface BigoConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

export function BigoConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: BigoConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={Lock} accent="sky">
          <Trans>Protection Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'stream_password')}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="sky">
                  <Trans>Stream Password</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                    placeholder={i18n._(msg`Password...`)}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>
                    Default password for password-protected rooms. Can be
                    overridden per streamer with ?pwd= in the URL.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>

      <section className="space-y-6">
        <ConfigSectionHeading icon={Shield} accent="sky">
          <Trans>API Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FlagFormField
            fieldName={configPath<TFieldValues>(fieldName, 'mint_token')}
            title={<Trans>Mint Integrity Token</Trans>}
            description={
              <Trans>
                Send a website-style integrity token with studio requests.
                Enabled by default; disable only if minting fails in your
                network.
              </Trans>
            }
            checked={(value) => value !== false && value !== 'false'}
          />
        </div>
      </section>
    </div>
  );
}
