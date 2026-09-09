import { get } from 'react-hook-form';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Tv } from 'lucide-react';
import { EndStreamOnDanmuCloseField } from '@/components/config/shared/end-stream-on-danmu-close-field';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface BilibiliConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
  inherited?: boolean;
}

// Keep these request codes aligned with the extractor's BilibiliQuality enum.
const QUALITY_OPTIONS = [
  { code: 30000, label: msg`Dolby Vision (30000)` },
  { code: 20000, label: msg`4K (20000)` },
  { code: 10000, label: msg`Original (10000)` },
  { code: 401, label: msg`Blu-ray Dolby (401)` },
  { code: 400, label: msg`Blu-ray (400)` },
  { code: 250, label: msg`Ultra (250)` },
  { code: 150, label: msg`HD (150)` },
  { code: 80, label: msg`Low (80)` },
  { code: 0, label: msg`Lowest (0)` },
];

export function BilibiliConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
  inherited = false,
}: BilibiliConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();
  const defaultLabel = inherited
    ? i18n._(msg`Inherited`)
    : i18n._(msg`Default: Dolby Vision (30000)`);

  return (
    <div className="space-y-12">
      {/* Extraction Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Tv} accent="indigo">
          <Trans>Extraction Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            // A controller retains its initial default as a fallback for undefined resets.
            // Recreate it when saved defaults change so clearing an override stays unset.
            key={
              JSON.stringify(
                get(form.formState.defaultValues, `${fieldName}.quality`),
              ) ?? 'unset'
            }
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'quality')}
            render={({ field }) => {
              const selected = QUALITY_OPTIONS.find(
                (option) => option.code === field.value,
              );
              return (
                <FormItem className="space-y-4">
                  <ConfigFieldLabel accent="indigo">
                    <Trans>Preferred Quality (QN)</Trans>
                  </ConfigFieldLabel>
                  <Select
                    onValueChange={(v) =>
                      field.onChange(v === 'default' ? null : Number(v))
                    }
                    value={field.value?.toString() ?? 'default'}
                  >
                    <FormControl>
                      <SelectTrigger className="bg-background/50 h-12 rounded-2xl border-border/50 focus:bg-background transition-all shadow-sm">
                        <SelectValue>
                          {field.value == null
                            ? defaultLabel
                            : selected
                              ? i18n._(selected.label)
                              : String(field.value)}
                        </SelectValue>
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent className="rounded-xl border-border/50 shadow-xl">
                      <SelectItem value="default">{defaultLabel}</SelectItem>
                      {QUALITY_OPTIONS.map((option) => (
                        <SelectItem
                          key={option.code}
                          value={String(option.code)}
                        >
                          {i18n._(option.label)}
                        </SelectItem>
                      ))}
                      {field.value != null && !selected && (
                        <SelectItem value={String(field.value)}>
                          {String(field.value)}
                        </SelectItem>
                      )}
                    </SelectContent>
                  </Select>
                  <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                    <Trans>
                      Select the highest quality level you want to attempt
                      capturing.
                    </Trans>
                  </FormDescription>
                </FormItem>
              );
            }}
          />

          <EndStreamOnDanmuCloseField
            form={form}
            name={configPath<TFieldValues>(
              fieldName,
              'end_stream_on_danmu_stream_closed',
            )}
          />
        </div>
      </section>
    </div>
  );
}
