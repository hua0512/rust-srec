import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Lock, Shield } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';

interface BigoConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function BigoConfigFields({ form, fieldName }: BigoConfigFieldsProps) {
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
            name={`${fieldName}.stream_password`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="sky">
                  <Trans>Stream Password</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="password"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                    placeholder={i18n._(msg`Password...`)}
                  />
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
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
          <FormField
            control={form.control}
            name={`${fieldName}.mint_token`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-background/40 px-4 py-3">
                <div className="space-y-1 pr-4">
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>Mint Integrity Token</Trans>
                  </FormLabel>
                  <FormDescription className="text-[11px] font-medium text-muted-foreground/80">
                    <Trans>
                      Send a website-style integrity token with studio requests.
                      Enabled by default; disable only if minting fails in your
                      network.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value !== false && field.value !== 'false'}
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
