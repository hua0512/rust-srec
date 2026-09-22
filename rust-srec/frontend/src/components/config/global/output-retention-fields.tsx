import { Trans } from '@lingui/react/macro';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { CONFIG_INPUT } from '@/components/config/shared/config-field';

export function OutputRetentionFields() {
  const { control } = useFormContext();
  const days = useWatch({ control, name: 'output_retention_days' });

  return (
    <>
      <FormField
        name="output_retention_days"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <FormLabel>
              <Trans>Output retention (days)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                {...field}
                className={CONFIG_INPUT}
                type="number"
                min={0}
                max={2147483647}
                step={1}
                value={field.value ?? 0}
                onChange={(event) => field.onChange(Number(event.target.value))}
              />
            </FormControl>
            <FormDescription>
              <Trans>
                Set to 0 to disable automatic output cleanup. Cleanup runs every
                30 minutes and only includes ended sessions with no active or
                recently updated processing jobs.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        name="output_retention_delete_files"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <FormLabel>
              <Trans>When outputs expire</Trans>
            </FormLabel>
            <Select
              value={field.value ? 'files' : 'records'}
              onValueChange={(value) => field.onChange(value === 'files')}
              disabled={!days}
            >
              <FormControl>
                <SelectTrigger onBlur={field.onBlur} ref={field.ref}>
                  <SelectValue />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem value="records">
                  <Trans>Delete records only</Trans>
                </SelectItem>
                <SelectItem value="files">
                  <Trans>Delete records and files</Trans>
                </SelectItem>
              </SelectContent>
            </Select>
            <FormDescription>
              {field.value ? (
                <Trans>
                  Permanently deletes tracked output files from disk. Files
                  still in use are kept for a later cleanup.
                </Trans>
              ) : (
                <Trans>
                  Removes outputs from the library but keeps their files on
                  disk. Once a record is removed, automatic cleanup can no
                  longer delete its file, even if you change this setting later.
                </Trans>
              )}
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    </>
  );
}
