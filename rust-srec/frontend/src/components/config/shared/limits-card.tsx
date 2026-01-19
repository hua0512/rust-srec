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
import { Shield } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { memo } from 'react';

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
      <CardContent className="grid grid-cols-1 sm:grid-cols-3 gap-6">
        <FormField
          control={form.control}
          name={
            basePath
              ? `${basePath}.max_download_duration_secs`
              : 'max_download_duration_secs'
          }
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Max Duration</Trans>
              </FormLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value ?? null}
                  onChange={field.onChange}
                  unitType="duration"
                  placeholder={i18n._(msg`Global Default`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
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
            <FormItem>
              <FormLabel>
                <Trans>Min Segment Size</Trans>
              </FormLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value ?? null}
                  onChange={field.onChange}
                  unitType="size"
                  placeholder={i18n._(msg`Global Default`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
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
            <FormItem>
              <FormLabel>
                <Trans>Max Part Size</Trans>
              </FormLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value ?? null}
                  onChange={field.onChange}
                  unitType="size"
                  placeholder={i18n._(msg`Global Default`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
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
