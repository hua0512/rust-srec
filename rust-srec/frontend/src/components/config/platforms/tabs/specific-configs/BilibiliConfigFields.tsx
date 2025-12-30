import { useEffect } from 'react';
import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';

interface BilibiliConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function BilibiliConfigFields({
  form,
  fieldName,
}: BilibiliConfigFieldsProps) {
  useEffect(() => {
    // If current value is undefined or null, initialized to 30000 (backend default)
    // explicitly in the form state so it's not empty {}
    const currentVal = form.getValues(`${fieldName}.quality`);
    if (currentVal === undefined) {
      form.setValue(`${fieldName}.quality`, 30000);
    }
  }, [form, fieldName]);

  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name={`${fieldName}.quality`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Preferred Quality (QN)</Trans>
            </FormLabel>
            <Select
              onValueChange={(val) => field.onChange(parseInt(val))}
              value={field.value?.toString() || '30000'}
            >
              <FormControl>
                <SelectTrigger>
                  <SelectValue placeholder="Select quality" />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem value="30000">Dolby Vision</SelectItem>
                <SelectItem value="20000">4K</SelectItem>
                <SelectItem value="10000">Original (原画)</SelectItem>
                <SelectItem value="401">Blue (Dolby Audio)</SelectItem>
                <SelectItem value="400">Blue (蓝光)</SelectItem>
                <SelectItem value="250">Ultra (超清)</SelectItem>
                <SelectItem value="150">Medium (高清)</SelectItem>
                <SelectItem value="80">Low (流畅)</SelectItem>
                <SelectItem value="0">Lowest</SelectItem>
              </SelectContent>
            </Select>
            <FormDescription>
              <Trans>Bilibili quality identification (QN).</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
    </div>
  );
}
