import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/react/macro';

interface TikTokConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function TikTokConfigFields({
  form,
  fieldName,
}: TikTokConfigFieldsProps) {
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
    </div>
  );
}
