import { memo } from 'react';
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
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Tv } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import {
  CONFIG_DESCRIPTION,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
} from './config-field';
import { cn } from '@/lib/utils';

interface RecordDanmuCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export const RecordDanmuCard = memo(
  ({ form, basePath }: RecordDanmuCardProps) => {
    const { i18n } = useLingui();
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
                  <FormLabel className="text-sm font-semibold">
                    <Trans>Capture Mode</Trans>
                  </FormLabel>
                  <FormDescription className={CONFIG_DESCRIPTION}>
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
                      <SelectTrigger
                        className={cn(CONFIG_SELECT_TRIGGER, 'w-[180px]')}
                      >
                        <SelectValue
                          placeholder={i18n._(msg`Select behavior`)}
                        />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent className={CONFIG_SELECT_CONTENT}>
                      <SelectItem value="null">
                        <Trans>Global Default</Trans>
                      </SelectItem>
                      <SelectItem value="true">
                        <Trans>Enabled</Trans>
                      </SelectItem>
                      <SelectItem value="false">
                        <Trans>Disabled</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormControl>
              </FormItem>
            )}
          />
        </CardContent>
      </Card>
    );
  },
);

RecordDanmuCard.displayName = 'RecordDanmuCard';
