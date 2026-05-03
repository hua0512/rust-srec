import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Timer } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { Input } from '@/components/ui/input';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { memo } from 'react';

interface OfflineCheckCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

// Per-platform / per-template / per-streamer overrides for the
// offline-confirmation cadence. Empty input → null = "inherit from parent".
// Server floors enforce count >= 1, delay_ms >= 1000.
export const OfflineCheckCard = memo(
  ({ form, basePath }: OfflineCheckCardProps) => {
    const { i18n } = useLingui();
    return (
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-orange-500/10 text-orange-600 dark:text-orange-400">
              <Timer className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Offline Check</Trans>
              </CardTitle>
              <p className="text-sm text-muted-foreground">
                <Trans>
                  Override how aggressively the scheduler confirms a stream has
                  ended.
                </Trans>
              </p>
            </div>
          </div>
        </CardHeader>
        <CardContent className="grid grid-cols-1 sm:grid-cols-2 gap-6">
          <FormField
            control={form.control}
            name={
              basePath
                ? `${basePath}.offline_check_delay_ms`
                : 'offline_check_delay_ms'
            }
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Offline Check Interval</Trans>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    unitType="duration"
                    value={
                      field.value !== null && field.value !== undefined
                        ? Number(field.value) / 1000
                        : null
                    }
                    onChange={(val) =>
                      field.onChange(val !== null ? val * 1000 : null)
                    }
                    placeholder={i18n._(msg`Inherited`)}
                    className="bg-background"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Interval between offline checks.</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name={
              basePath
                ? `${basePath}.offline_check_count`
                : 'offline_check_count'
            }
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Offline Detection Count</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    min={1}
                    value={field.value ?? ''}
                    onChange={(e) => {
                      const v = e.target.value;
                      field.onChange(v === '' ? null : Number(v));
                    }}
                    placeholder={i18n._(msg`Inherited`)}
                    className="bg-background"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>
                    Consecutive failed checks needed to confirm offline.
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

OfflineCheckCard.displayName = 'OfflineCheckCard';
