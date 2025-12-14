import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '../../../ui/form';

import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Clock } from 'lucide-react';
import { InputWithUnit } from '../../../ui/input-with-unit';
import { UseFormReturn } from 'react-hook-form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { RecordDanmuCard } from '../../shared/record-danmu-card';
import { OutputSettingsCard } from '../../shared/output-settings-card';
import { LimitsCard } from '../../shared/limits-card';

interface GeneralTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function GeneralTab({ form, basePath }: GeneralTabProps) {
  return (
    <div className="grid gap-6">
      <RecordDanmuCard form={form} basePath={basePath} />

      {/* Timing & Delays Card */}
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400">
              <Clock className="w-5 h-5" />
            </div>
            <CardTitle className="text-lg">
              <Trans>Timing & Delays</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="grid grid-cols-1 sm:grid-cols-2 gap-6">
          <FormField
            control={form.control}
            name={basePath ? `${basePath}.fetch_delay_ms` : 'fetch_delay_ms'}
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Fetch Delay</Trans>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    value={
                      field.value !== null && field.value !== undefined
                        ? field.value / 1000
                        : null
                    }
                    onChange={(v) =>
                      field.onChange(v !== null ? Math.round(v * 1000) : null)
                    }
                    unitType="duration"
                    placeholder={t`Global Default`}
                    className="bg-background"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Interval between checks.</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name={
              basePath ? `${basePath}.download_delay_ms` : 'download_delay_ms'
            }
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Download Delay</Trans>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    value={
                      field.value !== null && field.value !== undefined
                        ? field.value / 1000
                        : null
                    }
                    onChange={(v) =>
                      field.onChange(v !== null ? Math.round(v * 1000) : null)
                    }
                    unitType="duration"
                    placeholder={t`Global Default`}
                    className="bg-background"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Wait time before starting.</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>

      <OutputSettingsCard form={form} basePath={basePath} />
      <LimitsCard form={form} basePath={basePath} />
    </div>
  );
}
