import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
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

        <div className="space-y-6">
          <FormField
            control={form.control}
            name={`${fieldName}.api_mode`}
            render={({ field }) => (
              <FormItem>
                <div className="flex items-center gap-2 mb-3">
                  <div className="w-1.5 h-1.5 rounded-full bg-indigo-500" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>Extraction API Mode</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value || 'WEB'}
                  >
                    <SelectTrigger className="bg-background/50 h-11 rounded-xl border-border/50 focus:bg-background transition-all shadow-sm">
                      <SelectValue placeholder="Select API Mode" />
                    </SelectTrigger>
                    <SelectContent className="rounded-xl border-border/50 shadow-xl">
                      <SelectItem value="WEB">
                        <Trans>WEB</Trans>{' '}
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Default</Trans>)
                        </span>
                      </SelectItem>
                      <SelectItem value="MP">
                        <Trans>MP</Trans>
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Numeric Room IDs Only</Trans>)
                        </span>
                      </SelectItem>
                      <SelectItem value="WUP">
                        <Trans>WUP</Trans>{' '}
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Numeric Room IDs Only</Trans>)
                        </span>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    API protocol to use for live stream extraction. WUP is the
                    standard protocol for the PC app. Note that WUP and MP only
                    work with numeric room IDs.
                  </Trans>
                </FormDescription>
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
