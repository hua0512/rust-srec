import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '../../../ui/form';
import { Textarea } from '../../../ui/textarea';
import { Trans } from '@lingui/react/macro';
import { Cookie } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

interface AuthTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function AuthTab({ form, basePath }: AuthTabProps) {
  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-orange-500/10 text-orange-600 dark:text-orange-400">
            <Cookie className="w-5 h-5" />
          </div>
          <div className="space-y-1">
            <CardTitle className="text-lg">
              <Trans>Authentication Cookies</Trans>
            </CardTitle>
            <p className="text-sm text-muted-foreground">
              <Trans>Required for premium/login-only content.</Trans>
            </p>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <FormField
          control={form.control}
          name={basePath ? `${basePath}.cookies` : 'cookies'}
          render={({ field }) => (
            <FormItem>
              <FormControl>
                <Textarea
                  placeholder="key=value; key2=value2"
                  className="font-mono text-xs min-h-[200px] bg-muted/30 resize-y"
                  {...field}
                  value={field.value ?? ''}
                  onChange={(e) => field.onChange(e.target.value || null)}
                />
              </FormControl>
              <FormDescription>
                <Trans>
                  Paste your Netscape formatted cookies or raw key=value string
                  here.
                </Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </CardContent>
    </Card>
  );
}
