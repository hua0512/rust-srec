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
import { Zap, Cloud, Gamepad2, RotateCcw } from 'lucide-react';

interface DouyuConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function DouyuConfigFields({ form, fieldName }: DouyuConfigFieldsProps) {
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
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={form.control}
              name={`${fieldName}.cdn`}
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-center gap-2 mb-3">
                    <Cloud className="w-4 h-4 text-muted-foreground" />
                    <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                      <Trans>Preferred CDN</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Input
                      placeholder="ws-h5, hs-h5, etc."
                      {...field}
                      value={field.value || 'ws-h5'}
                      className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all"
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] font-medium pt-1 px-1">
                    <Trans>
                      Specify preferred content delivery network (e.g., ws-h5,
                      hs-h5).
                    </Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name={`${fieldName}.rate`}
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-center gap-2 mb-3">
                    <div className="w-1.5 h-1.5 rounded-full bg-indigo-500" />
                    <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                      <Trans>Quality Rate</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      onChange={(e) =>
                        field.onChange(parseInt(e.target.value) || 0)
                      }
                      value={field.value ?? 0}
                      className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all"
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] font-medium pt-1">
                    <Trans>Use 0 for original quality.</Trans>
                  </FormDescription>
                </FormItem>
              )}
            />
          </div>

          <FormField
            control={form.control}
            name={`${fieldName}.force_hs`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                <div className="space-y-0.5">
                  <FormLabel className="text-xs font-bold text-foreground">
                    <Trans>Force Huoshan CDN</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px] leading-tight font-medium text-muted-foreground/80">
                    <Trans>Always prioritize Huoshan delivery nodes.</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                    className="scale-90"
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>
      </section>

      {/* Network & Content Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Gamepad2 className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Network & Content</Trans>
          </h4>
        </div>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.disable_interactive_game`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                <div className="space-y-0.5">
                  <FormLabel className="text-xs font-bold text-foreground">
                    <Trans>Filter Interactive Games</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px] leading-tight font-medium text-muted-foreground/80">
                    <Trans>
                      Treat interactive games as offline platforms during
                      extraction checks.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                    className="scale-90"
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={`${fieldName}.request_retries`}
            render={({ field }) => (
              <FormItem>
                <div className="flex items-center gap-2 mb-3">
                  <RotateCcw className="w-4 h-4 text-muted-foreground" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>API Request Retries</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) =>
                      field.onChange(parseInt(e.target.value) || 0)
                    }
                    value={field.value ?? 3}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all max-w-[120px]"
                  />
                </FormControl>
                <FormDescription className="text-[10px] font-medium pt-1">
                  <Trans>
                    Max number of retry attempts for metadata fetching.
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
