import { useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { TagInput } from '@/components/ui/tag-input';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/macro';

export function CategoryFilterForm() {
  const { control } = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={control}
        name="config.categories"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Categories</Trans>
            </FormLabel>
            <FormControl>
              <TagInput
                {...field}
                placeholder="Enter categories..."
                value={field.value || []}
                onChange={(newTags) => field.onChange(newTags)}
              />
            </FormControl>
            <FormDescription>
              <Trans>
                Enter categories to match against (e.g. Just Chatting, Gaming).
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
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Exclude</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>
                  If enabled, streams in these categories will be ignored.
                </Trans>
              </FormDescription>
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
