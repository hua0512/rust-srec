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
import { Lock } from 'lucide-react';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';

interface TwitcastingConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function TwitcastingConfigFields({
  form,
  fieldName,
}: TwitcastingConfigFieldsProps) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-12">
      {/* Protection Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Lock} accent="indigo">
          <Trans>Protection Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.password`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="indigo">
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
                    Required if the stream is password-protected by the
                    broadcaster.
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
