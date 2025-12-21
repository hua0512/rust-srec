// ... imports
import { Control } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Database, Wrench, Film } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

interface MesioFormProps {
  control: Control<any>;
  basePath?: string;
}

export function MesioForm({ control, basePath = 'config' }: MesioFormProps) {
  return (
    <div className="space-y-6">
      <Card className="border-border/40 bg-background/40 shadow-sm">
        <CardHeader className="pb-3 pt-4 px-4">
          <CardTitle className="text-sm font-medium flex items-center gap-2">
            <Database className="w-4 h-4 text-primary" />
            <Trans>Buffer Settings</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="px-4 pb-4">
          <FormField
            control={control}
            name={`${basePath}.buffer_size`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                  <Trans>Buffer Size</Trans>
                </FormLabel>
                <FormControl>
                  <div className="flex items-center gap-2">
                    <Input
                      type="number"
                      {...field}
                      className="bg-background/50"
                    />
                    <span className="text-xs text-muted-foreground whitespace-nowrap">
                      <Trans>bytes</Trans>
                    </span>
                  </div>
                </FormControl>
                <FormDescription className="text-[10px]">
                  <Trans>Default: 8388608 (8 MiB)</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <FormField
          control={control}
          name={`${basePath}.fix_flv`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-background/40 p-4 shadow-sm transition-colors">
              <div className="space-y-0.5">
                <FormLabel className="text-sm font-medium flex items-center gap-2">
                  <Film className="w-4 h-4 text-orange-500" />
                  <Trans>Fix FLV</Trans>
                </FormLabel>
                <FormDescription className="text-[10px]">
                  <Trans>Attempt to repair timestamps in FLV streams</Trans>
                </FormDescription>
              </div>
              <FormControl>
                <Switch
                  checked={field.value}
                  onCheckedChange={field.onChange}
                />
              </FormControl>
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${basePath}.fix_hls`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-background/40 p-4 shadow-sm transition-colors">
              <div className="space-y-0.5">
                <FormLabel className="text-sm font-medium flex items-center gap-2">
                  <Wrench className="w-4 h-4 text-blue-500" />
                  <Trans>Fix HLS</Trans>
                </FormLabel>
                <FormDescription className="text-[10px]">
                  <Trans>Handle discontinuities in HLS streams</Trans>
                </FormDescription>
              </div>
              <FormControl>
                <Switch
                  checked={field.value}
                  onCheckedChange={field.onChange}
                />
              </FormControl>
            </FormItem>
          )}
        />
      </div>
    </div>
  );
}
