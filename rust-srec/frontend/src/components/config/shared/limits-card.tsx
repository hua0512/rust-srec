import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Shield } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { memo } from 'react';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  ConfigFieldLabel,
} from './config-field';

interface LimitsCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export const LimitsCard = memo(({ form, basePath }: LimitsCardProps) => {
  const { i18n } = useLingui();
  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-red-500/10 text-red-600 dark:text-red-400">
            <Shield className="w-5 h-5" />
          </div>
          <div className="space-y-1">
            <CardTitle className="text-lg">
              <Trans>Limits & Validation</Trans>
            </CardTitle>
            <p className="text-sm text-muted-foreground">
              <Trans>Set constraints on downloads.</Trans>
            </p>
          </div>
        </div>
      </CardHeader>
      <CardContent className="grid grid-cols-1 gap-6 sm:grid-cols-2 2xl:grid-cols-3">
        <FormField
          control={form.control}
          name={
            basePath
              ? `${basePath}.max_download_duration_secs`
              : 'max_download_duration_secs'
          }
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Duration</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value ?? null}
                  onChange={field.onChange}
                  unitType="duration"
                  placeholder={i18n._(msg`Global Default`)}
                  className={CONFIG_INPUT}
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Split after duration.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name={
            basePath
              ? `${basePath}.min_segment_size_bytes`
              : 'min_segment_size_bytes'
          }
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Min Segment Size</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value ?? null}
                  onChange={field.onChange}
                  unitType="size"
                  placeholder={i18n._(msg`Global Default`)}
                  className={CONFIG_INPUT}
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Min size to keep.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name={
            basePath ? `${basePath}.max_part_size_bytes` : 'max_part_size_bytes'
          }
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Part Size</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value ?? null}
                  onChange={field.onChange}
                  unitType="size"
                  placeholder={i18n._(msg`Global Default`)}
                  className={CONFIG_INPUT}
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Split after size.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </CardContent>
    </Card>
  );
});

LimitsCard.displayName = 'LimitsCard';
