import { UseFormReturn } from 'react-hook-form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Textarea } from '@/components/ui/textarea';
import { Trans } from '@lingui/react/macro';
import { Cookie, Network } from 'lucide-react';
import { RetryPolicyForm } from './retry-policy-form';

interface NetworkSettingsCardProps {
  form: UseFormReturn<any>;
  paths: {
    cookies: string;
    retryPolicy: string;
  };
  configMode?: 'json' | 'object';
}

export function NetworkSettingsCard({
  form,
  paths,
  configMode = 'object',
}: NetworkSettingsCardProps) {
  return (
    <div className="grid gap-6">
      {/* Authentication Card */}
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-orange-500/10 text-orange-600 dark:text-orange-400">
              <Cookie className="w-5 h-5" />
            </div>
            <CardTitle className="text-lg">
              <Trans>Authentication</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          <FormField
            control={form.control}
            name={paths.cookies}
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Cookies</Trans>
                </FormLabel>
                <FormControl>
                  <Textarea
                    {...field}
                    placeholder="key=value; key2=value2"
                    value={field.value ?? ''}
                    className="font-mono text-sm bg-background min-h-[100px]"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>
                    HTTP cookies for authentication (Result of document.cookie).
                  </Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>

      {/* Retry Policy Card */}
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-green-500/10 text-green-600 dark:text-green-400">
              <Network className="w-5 h-5" />
            </div>
            <CardTitle className="text-lg">
              <Trans>Download Retry Policy</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          <RetryPolicyForm
            form={form}
            name={paths.retryPolicy}
            mode={configMode}
          />
        </CardContent>
      </Card>
    </div>
  );
}
