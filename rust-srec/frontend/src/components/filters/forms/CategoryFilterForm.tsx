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
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

export function CategoryFilterForm() {
  const { i18n } = useLingui();
  const { control } = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={control}
        name="config.categories"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Categories</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <TagInput
                {...field}
                placeholder={i18n._(msg`Enter categories...`)}
                value={field.value || []}
                onChange={(newTags) => field.onChange(newTags)}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
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
              <FormDescription className={CONFIG_DESCRIPTION}>
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
