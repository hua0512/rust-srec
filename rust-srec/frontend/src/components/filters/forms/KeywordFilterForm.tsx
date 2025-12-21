import { useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/macro';
import { TagInput } from '@/components/ui/tag-input';

export function KeywordFilterForm() {
  const { control } = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={control}
        name="config.keywords"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Keywords</Trans>
            </FormLabel>
            <FormControl>
              <TagInput
                {...field}
                placeholder="Enter keywords..."
                value={field.value || []}
                onChange={(newValue) => field.onChange(newValue)}
              />
            </FormControl>
            <FormDescription>
              <Trans>Enter keywords to match against the stream title.</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name="config.exclude"
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Exclude</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>
                  If enabled, streams matching these keywords will be ignored.
                </Trans>
              </FormDescription>
            </div>
            <FormControl>
              <Switch checked={field.value} onCheckedChange={field.onChange} />
            </FormControl>
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name="config.case_sensitive"
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Case Sensitive</Trans>
              </FormLabel>
            </div>
            <FormControl>
              <Switch checked={field.value} onCheckedChange={field.onChange} />
            </FormControl>
          </FormItem>
        )}
      />
    </div>
  );
}
