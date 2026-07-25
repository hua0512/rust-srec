import { UseFormReturn } from 'react-hook-form';
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
import { Key, Lock } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';

interface SoopConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function SoopConfigFields({ form, fieldName }: SoopConfigFieldsProps) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={Key} accent="emerald">
          <Trans>Authentication</Trans>
        </ConfigSectionHeading>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.username`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="emerald">
                  <Trans>Username</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="text"
                    autoComplete="off"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all"
                    placeholder="example_user"
                  />
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                  <Trans>
                    SOOP account used to watch login-required (e.g. 19+)
                    broadcasts. Prefer cookies for permanently restricted
                    channels.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={`${fieldName}.password`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="emerald">
                  <Trans>Password</Trans>
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
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                  <Trans>
                    SOOP account used to watch login-required (e.g. 19+)
                    broadcasts. Prefer cookies for permanently restricted
                    channels.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>

      <section className="space-y-6">
        <ConfigSectionHeading icon={Lock} accent="emerald">
          <Trans>Stream Password</Trans>
        </ConfigSectionHeading>

        <FormField
          control={form.control}
          name={`${fieldName}.stream_password`}
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
              <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
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
