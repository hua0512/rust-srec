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
import { Zap, Activity } from 'lucide-react';

interface HuyaConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function HuyaConfigFields({ form, fieldName }: HuyaConfigFieldsProps) {
  return (
    <div className="space-y-12">
      {/* Protocol Settings Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Zap className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Protocol Settings</Trans>
          </h4>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <FormField
            control={form.control}
            name={`${fieldName}.use_wup`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                <div className="space-y-0.5 pr-4">
                  <FormLabel className="text-xs font-bold text-foreground">
                    <Trans>Use WUP Protocol</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px] leading-tight font-medium">
                    <Trans>
                      Standard protocol for extraction. Recommended for most
                      cases.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value ?? true}
                    onCheckedChange={(checked) => {
                      field.onChange(checked);
                      if (checked) {
                        form.setValue(`${fieldName}.use_wup_v2`, false);
                      }
                    }}
                    className="scale-90"
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={`${fieldName}.use_wup_v2`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                <div className="space-y-0.5 pr-4">
                  <FormLabel className="text-xs font-bold text-foreground">
                    <Trans>Use WUP V2</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px] leading-tight font-medium">
                    <Trans>
                      Alternative protocol variant for live stream extraction.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={(checked) => {
                      field.onChange(checked);
                      if (checked) {
                        form.setValue(`${fieldName}.use_wup`, false);
                      }
                    }}
                    className="scale-90"
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>
      </section>

      {/* Quality Settings Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Activity className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Quality Settings</Trans>
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
                      Force requesting the highest origin quality available
                      (Direct stream).
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
