import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '../../../ui/form';
import { UseFormReturn } from 'react-hook-form';
import { ProxyConfigSettings } from '../../proxy-config-settings';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../../../ui/select';
import { Trans } from '@lingui/react/macro';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Shield } from 'lucide-react';

interface ProxyTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function ProxyTab({ form, basePath }: ProxyTabProps) {
  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-green-500/10 text-green-600 dark:text-green-400">
            <Shield className="w-5 h-5" />
          </div>
          <div className="space-y-1">
            <CardTitle className="text-lg">
              <Trans>Proxy Configuration</Trans>
            </CardTitle>
            <p className="text-sm text-muted-foreground">
              <Trans>Manage network proxy settings for this platform.</Trans>
            </p>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <FormField
          control={form.control}
          name={basePath ? `${basePath}.proxy_config` : 'proxy_config'}
          render={({ field }) => {
            const isInherited =
              field.value === null || field.value === undefined;

            return (
              <FormItem className="space-y-6">
                <div className="flex items-center justify-between p-4 rounded-xl border bg-muted/30">
                  <div className="space-y-0.5">
                    <FormLabel className="text-base font-semibold">
                      <Trans>Proxy Strategy</Trans>
                    </FormLabel>
                    <FormDescription>
                      <Trans>
                        Choose how this platform handles proxy connections.
                      </Trans>
                    </FormDescription>
                  </div>
                  <Select
                    value={isInherited ? 'inherit' : 'custom'}
                    onValueChange={(v) => {
                      if (v === 'inherit') {
                        field.onChange(null);
                      } else {
                        // Initialize with disabled proxy if coming from null
                        field.onChange(
                          JSON.stringify({
                            enabled: false,
                            use_system_proxy: false,
                          }),
                        );
                      }
                    }}
                  >
                    <FormControl>
                      <SelectTrigger className="w-[200px] bg-background">
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="inherit">
                        <Trans>Global Default</Trans>
                      </SelectItem>
                      <SelectItem value="custom">
                        <Trans>Custom Configuration</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                {!isInherited && (
                  <div className="animate-in fade-in-50 slide-in-from-top-2 pt-2">
                    <ProxyConfigSettings
                      value={field.value}
                      onChange={field.onChange}
                    />
                  </div>
                )}
                <FormMessage />
              </FormItem>
            );
          }}
        />
      </CardContent>
    </Card>
  );
}
