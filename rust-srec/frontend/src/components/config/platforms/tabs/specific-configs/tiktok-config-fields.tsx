import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/react/macro';
import { Zap } from 'lucide-react';
import { ConfigSectionHeading } from '@/components/config/shared/config-field';

interface TikTokConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function TikTokConfigFields({
  form,
  fieldName,
}: TikTokConfigFieldsProps) {
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
            name={`${fieldName}.force_origin_quality`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-2xl border bg-muted/5 p-5 transition-all hover:bg-muted/10 border-border/50">
                <div className="space-y-1.5 pr-4">
                  <FormLabel className="text-sm font-bold text-foreground">
                    <Trans>Force Origin Quality</Trans>
                  </FormLabel>
                  <FormDescription className="text-xs leading-relaxed font-medium">
                    <Trans>
                      Attempt to get the highest original quality available.
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
