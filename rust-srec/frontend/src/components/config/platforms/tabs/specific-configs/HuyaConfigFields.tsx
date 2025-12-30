import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/react/macro';

interface HuyaConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function HuyaConfigFields({ form, fieldName }: HuyaConfigFieldsProps) {
  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name={`${fieldName}.use_wup`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Use WUP Protocol</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>
                  Use WUP protocol for extraction. Recommended for most cases.
                </Trans>
              </FormDescription>
            </div>
            <FormControl>
              <Switch
                checked={field.value ?? true}
                onCheckedChange={(checked) => {
                  field.onChange(checked);
                  if (checked) {
                    form.setValue(`${fieldName}.use_wup_v2`, false);
                  }
                }}
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name={`${fieldName}.use_wup_v2`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Use WUP V2</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>
                  Use WUP protocol for live stream extraction. Only available
                  for numeric room IDs.
                </Trans>
              </FormDescription>
            </div>
            <FormControl>
              <Switch
                checked={!!field.value}
                onCheckedChange={(checked) => {
                  field.onChange(checked);
                  if (checked) {
                    form.setValue(`${fieldName}.use_wup`, false);
                  }
                }}
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name={`${fieldName}.force_origin_quality`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Force Origin Quality</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>Force requesting the highest origin quality.</Trans>
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
    </div>
  );
}
