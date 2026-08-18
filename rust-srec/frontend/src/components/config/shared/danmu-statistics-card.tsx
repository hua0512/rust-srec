import { memo } from 'react';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { ChartColumn } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  ConfigFieldLabel,
} from './config-field';

interface DanmuStatisticsCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

/**
 * Per-session danmu statistics settings.
 *
 * A layer either states the whole object or inherits it, so an empty numeric
 * input leaves that field to the backend's default rather than writing a zero.
 * Values are clamped server-side, which is why the hints give ranges rather than
 * the inputs enforcing them.
 */
export const DanmuStatisticsCard = memo(
  ({ form, basePath }: DanmuStatisticsCardProps) => {
    const { i18n } = useLingui();
    const path = (field: string) =>
      basePath
        ? `${basePath}.danmu_statistics.${field}`
        : `danmu_statistics.${field}`;

    const numberField = (
      field: string,
      label: React.ReactNode,
      description: React.ReactNode,
      placeholder: string,
    ) => (
      <FormField
        control={form.control}
        name={path(field)}
        render={({ field: formField }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>{label}</ConfigFieldLabel>
            <FormControl>
              <Input
                type="number"
                min={1}
                className={CONFIG_INPUT}
                placeholder={placeholder}
                value={formField.value ?? ''}
                onChange={(event) =>
                  formField.onChange(
                    event.target.value === ''
                      ? null
                      : Number(event.target.value),
                  )
                }
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              {description}
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    );

    return (
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400">
              <ChartColumn className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Danmu Statistics</Trans>
              </CardTitle>
              <p className="text-sm text-muted-foreground">
                <Trans>
                  How chat activity is summarised on the session page.
                </Trans>
              </p>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-6">
          <FormField
            control={form.control}
            name={path('enabled')}
            render={({ field }) => (
              <FormItem className="flex items-center justify-between gap-4">
                <div className="space-y-1">
                  <ConfigFieldLabel>
                    <Trans>Collect statistics</Trans>
                  </ConfigFieldLabel>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>
                      Chat files are still recorded when this is off; only the
                      per-session summary, which stores viewer names, is
                      skipped.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value ?? true}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-6">
            {numberField(
              'top_talkers',
              <Trans>Top chatters to keep</Trans>,
              <Trans>How many appear in the ranking. 1–500.</Trans>,
              '100',
            )}
            {numberField(
              'top_words',
              <Trans>Frequent words to keep</Trans>,
              <Trans>How many appear in the word chart. 1–500.</Trans>,
              '50',
            )}
            {numberField(
              'rate_bucket_secs',
              <Trans>Activity resolution (seconds)</Trans>,
              <Trans>
                Timeline granularity. Very long streams are automatically
                coarsened past a point.
              </Trans>,
              '10',
            )}
            {numberField(
              'talker_capacity',
              <Trans>Chatters tracked</Trans>,
              <Trans>
                Counts stay exact while a stream has fewer distinct chatters
                than this. 64–8192.
              </Trans>,
              '2048',
            )}
          </div>

          <FormField
            control={form.control}
            name={path('extra_stop_words')}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel>
                  <Trans>Ignored words</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Textarea
                    rows={3}
                    placeholder={i18n._(msg`One word per line`)}
                    value={
                      Array.isArray(field.value) ? field.value.join('\n') : ''
                    }
                    onChange={(event) => {
                      const words = event.target.value
                        .split('\n')
                        .map((word) => word.trim())
                        .filter(Boolean);
                      field.onChange(words.length > 0 ? words : null);
                    }}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>
                    Excluded from the frequent-words chart, on top of the
                    built-in list. One per line.
                  </Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>
    );
  },
);

DanmuStatisticsCard.displayName = 'DanmuStatisticsCard';
