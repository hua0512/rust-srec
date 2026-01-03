import { useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Trans } from '@lingui/macro';
import { TagInput } from '@/components/ui/tag-input';

export function KeywordFilterForm() {
  const { control } = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={control}
        name="config.include"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Include Keywords</Trans>
            </FormLabel>
            <FormControl>
              <TagInput
                {...field}
                placeholder="Enter keywords to include..."
                value={field.value || []}
                onChange={(newValue) => field.onChange(newValue)}
              />
            </FormControl>
            <FormDescription>
              <Trans>
                If set, stream titles must contain at least one of these
                keywords.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name="config.exclude"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Exclude Keywords</Trans>
            </FormLabel>
            <FormControl>
              <TagInput
                {...field}
                placeholder="Enter keywords to exclude..."
                value={field.value || []}
                onChange={(newValue) => field.onChange(newValue)}
              />
            </FormControl>
            <FormDescription>
              <Trans>
                If any of these keywords appear in the title, the stream will be
                ignored.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  );
}
