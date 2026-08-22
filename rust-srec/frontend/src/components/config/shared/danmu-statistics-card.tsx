import { memo, useEffect, useState } from 'react';
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
import { UseFormReturn, useWatch } from 'react-hook-form';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  ConfigFieldLabel,
} from './config-field';

interface DanmuStatisticsCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

/** One word per line, trimmed, blanks dropped. */
function toWords(text: string): string[] {
  return text
    .split('\n')
    .map((word) => word.trim())
    .filter(Boolean);
}

/**
 * Editor for the ignored-words list.
 *
 * The value is stored as a word array but edited as text, and mid-edit those
 * shapes disagree: a trailing newline, a blank line, and leading spaces are all
 * normal states while typing that normalize away. So the text is held here and
 * only converted on the way into the form. Deriving the textarea's value back
 * from the array instead would erase the newline that starts the next entry the
 * moment it was typed, making a second word impossible to enter.
 *
 * Trimming is left to the backend's `sanitized()`, which does it on save.
 */
function IgnoredWordsField({
  form,
  name,
}: {
  form: UseFormReturn<any>;
  name: string;
}) {
  const { i18n } = useLingui();
  const words = useWatch({ control: form.control, name }) as
    | string[]
    | undefined;
  const [draft, setDraft] = useState(() => (words ?? []).join('\n'));

  useEffect(() => {
    // Adopt the form value only when it changed from outside this field —
    // loading a config, or a form reset. An edit of our own already produced
    // this array, and re-deriving from it would discard the draft's whitespace.
    const fromForm = (words ?? []).join('\n');
    if (fromForm !== toWords(draft).join('\n')) {
      setDraft(fromForm);
    }
  }, [words, draft]);

  return (
    <FormField
      control={form.control}
      name={name}
      render={({ field }) => (
        <FormItem className="space-y-2">
          <ConfigFieldLabel>
            <Trans>Ignored words</Trans>
          </ConfigFieldLabel>
          <FormControl>
            <Textarea
              rows={3}
              placeholder={i18n._(msg`One word per line`)}
              value={draft}
              onChange={(event) => {
                const text = event.target.value;
                setDraft(text);
                const next = toWords(text);
                // Undefined rather than an empty array: an absent field means
                // this layer inherits, which is what clearing the box should do.
                field.onChange(next.length > 0 ? next : undefined);
              }}
              onBlur={field.onBlur}
            />
          </FormControl>
          <FormDescription className={CONFIG_DESCRIPTION}>
            <Trans>
              Excluded from the frequent-words chart, on top of the built-in
              list. One per line.
            </Trans>
          </FormDescription>
          <FormMessage />
        </FormItem>
      )}
    />
  );
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
                      ? undefined
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

          <IgnoredWordsField form={form} name={path('extra_stop_words')} />
        </CardContent>
      </Card>
    );
  },
);

DanmuStatisticsCard.displayName = 'DanmuStatisticsCard';
