import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';

interface TwitcastingConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function TwitcastingConfigFields({
  form,
  fieldName,
}: TwitcastingConfigFieldsProps) {
  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name={`${fieldName}.password`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Password</Trans>
            </FormLabel>
            <FormControl>
              <Input type="password" {...field} value={field.value || ''} />
            </FormControl>
            <FormDescription>
              <Trans>Password for protected streams.</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
    </div>
  );
}
