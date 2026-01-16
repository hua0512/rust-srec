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
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Zap className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Extraction Settings</Trans>
          </h4>
        </div>

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
