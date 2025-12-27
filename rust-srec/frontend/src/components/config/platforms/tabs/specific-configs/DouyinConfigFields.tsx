import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';

interface DouyinConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function DouyinConfigFields({
  form,
  fieldName,
}: DouyinConfigFieldsProps) {
  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name={`${fieldName}.force_origin_quality`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Force Origin Quality</Trans>
              </FormLabel>
            </div>
            <FormControl>
              <Switch
                checked={!!field.value}
                onCheckedChange={field.onChange}
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name={`${fieldName}.double_screen`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Double Screen Data</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>Use separate stream data for double screen.</Trans>
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
      <FormField
        control={form.control}
        name={`${fieldName}.ttwid_management_mode`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>TTWID Management Mode</Trans>
            </FormLabel>
            <Select
              onValueChange={field.onChange}
              value={field.value || 'global'}
            >
              <FormControl>
                <SelectTrigger>
                  <SelectValue placeholder="Select mode" />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem value="global">
                  <Trans>Global</Trans>
                </SelectItem>
                <SelectItem value="per_extractor">
                  <Trans>Per Extractor</Trans>
                </SelectItem>
              </SelectContent>
            </Select>
            <FormDescription>
              <Trans>How TTWID cookies are managed.</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name={`${fieldName}.ttwid`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>TTWID Cookie</Trans>
            </FormLabel>
            <FormControl>
              <Input
                placeholder="ttwid=..."
                {...field}
                value={field.value || ''}
              />
            </FormControl>
            <FormDescription>
              <Trans>Specific TTWID value for this platform.</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
    </div>
  );
}
