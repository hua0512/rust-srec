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

interface TwitchConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function TwitchConfigFields({
  form,
  fieldName,
}: TwitchConfigFieldsProps) {
  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name={`${fieldName}.oauth_token`}
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>OAuth Token</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="password"
                autoComplete="off"
                {...field}
                value={field.value || ''}
              />
            </FormControl>
            <FormDescription>
              <Trans>Twitch OAuth token for subscriber-only streams.</Trans>
            </FormDescription>
          </FormItem>
        )}
      />
    </div>
  );
}
