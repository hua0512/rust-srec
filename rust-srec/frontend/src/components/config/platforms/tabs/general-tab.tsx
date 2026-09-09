import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';

import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Clock } from 'lucide-react';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import type { FieldValues, UseFormReturn } from 'react-hook-form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { CONFIG_INPUT } from '@/components/config/shared/config-field';
import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface GeneralTabProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  basePath?: string;
}

export function GeneralTab<TFieldValues extends FieldValues>({
  form,
  basePath,
}: GeneralTabProps<TFieldValues>) {
  const { i18n } = useLingui();
  return (
    <div className="grid gap-6">
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
            name={configPath<TFieldValues>(basePath, 'fetch_delay_ms')}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel>
                  <Trans>Fetch Delay</Trans>
                </ConfigFieldLabel>
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
                    placeholder={i18n._(msg`Global Default`)}
                    className={CONFIG_INPUT}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>Interval between checks.</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(basePath, 'download_delay_ms')}
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel>
                  <Trans>Download Delay</Trans>
                </ConfigFieldLabel>
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
                    placeholder={i18n._(msg`Global Default`)}
                    className={CONFIG_INPUT}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>Wait time before starting.</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>
    </div>
  );
}
