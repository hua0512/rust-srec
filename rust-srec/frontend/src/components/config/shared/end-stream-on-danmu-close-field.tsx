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

interface EndStreamOnDanmuCloseFieldProps {
  form: UseFormReturn<any>;
  name: string;
}

export function EndStreamOnDanmuCloseField({
  form,
  name,
}: EndStreamOnDanmuCloseFieldProps) {
  return (
    <FormField
      control={form.control}
      name={name}
      render={({ field }) => (
        <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
          <div className="space-y-0.5 pr-4">
            <FormLabel className="text-xs font-bold text-foreground">
              <Trans>End Stream On Danmu Close</Trans>
            </FormLabel>
            <FormDescription className="text-[10px] leading-tight font-medium">
              <Trans>
                Stop recording when danmu control signals stream closed.
              </Trans>
            </FormDescription>
          </div>
          <FormControl>
            <Switch
              checked={field.value ?? true}
              onCheckedChange={field.onChange}
              className="scale-90"
            />
          </FormControl>
        </FormItem>
      )}
    />
  );
}
