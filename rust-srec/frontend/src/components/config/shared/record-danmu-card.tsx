import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Tv } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';

interface RecordDanmuCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function RecordDanmuCard({ form, basePath }: RecordDanmuCardProps) {
  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader>
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-green-500/10 text-green-600 dark:text-green-400">
            <Tv className="w-5 h-5" />
          </div>
          <div className="space-y-1">
            <CardTitle className="text-lg">
              <Trans>Record Danmu</Trans>
            </CardTitle>
            <p className="text-sm text-muted-foreground">
              <Trans>
                Capture real-time comments and chat messages if available.
              </Trans>
            </p>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <FormField
          control={form.control}
          name={basePath ? `${basePath}.record_danmu` : 'record_danmu'}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-xl border p-4 shadow-sm bg-muted/30">
              <div className="space-y-0.5">
                <FormLabel className="text-base font-medium">
                  <Trans>Capture Mode</Trans>
                </FormLabel>
                <FormDescription>
                  <Trans>Override global default.</Trans>
                </FormDescription>
              </div>
              <FormControl>
                <Select
                  value={
                    field.value === null || field.value === undefined
                      ? 'null'
                      : field.value
                        ? 'true'
                        : 'false'
                  }
                  onValueChange={(v) => {
                    if (v === 'null') field.onChange(null);
                    else if (v === 'true') field.onChange(true);
                    else field.onChange(false);
                  }}
                >
                  <FormControl>
                    <SelectTrigger className="w-[180px] bg-background">
                      <SelectValue placeholder={t`Select behavior`} />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="null">Global Default</SelectItem>
                    <SelectItem value="true">Enabled</SelectItem>
                    <SelectItem value="false">Disabled</SelectItem>
                  </SelectContent>
                </Select>
              </FormControl>
            </FormItem>
          )}
        />
      </CardContent>
    </Card>
  );
}
