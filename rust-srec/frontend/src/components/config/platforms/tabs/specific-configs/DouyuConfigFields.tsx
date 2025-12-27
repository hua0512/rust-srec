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
import { Trans } from '@lingui/react/macro';

interface DouyuConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function DouyuConfigFields({ form, fieldName }: DouyuConfigFieldsProps) {
  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name={`${fieldName}.cdn`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Preferred CDN</Trans>
            </FormLabel>
            <FormControl>
              <Input
                placeholder="ws-h5, hs-h5, etc."
                {...field}
                value={field.value || 'ws-h5'}
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name={`${fieldName}.disable_interactive_game`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Disable Interactive Games</Trans>
              </FormLabel>
              <FormDescription>
                <Trans>
                  Treat interactive games as offline to avoid recording if not
                  desired.
                </Trans>
              </FormDescription>
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
        name={`${fieldName}.rate`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Quality Rate</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                onChange={(e) => field.onChange(parseInt(e.target.value) || 0)}
                value={field.value ?? 0}
              />
            </FormControl>
            <FormDescription>
              <Trans>0 for original quality.</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name={`${fieldName}.force_hs`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
            <div className="space-y-0.5">
              <FormLabel className="text-base">
                <Trans>Force Huoshan CDN</Trans>
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
        name={`${fieldName}.request_retries`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>API Request Retries</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                onChange={(e) => field.onChange(parseInt(e.target.value) || 0)}
                value={field.value ?? 3}
              />
            </FormControl>
            <FormDescription>
              <Trans>Number of retries for API requests.</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
    </div>
  );
}
