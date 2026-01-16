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
import { Key } from 'lucide-react';

interface TwitchConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function TwitchConfigFields({
  form,
  fieldName,
}: TwitchConfigFieldsProps) {
  return (
    <div className="space-y-12">
      {/* Authentication Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Key className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Authentication</Trans>
          </h4>
        </div>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.oauth_token`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <div className="flex items-center gap-2 px-1">
                  <div className="w-1.5 h-1.5 rounded-full bg-indigo-500" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>OAuth Token</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Input
                    type="password"
                    autoComplete="off"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                    placeholder="oauth:..."
                  />
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                  <Trans>
                    Twitch OAuth token for subscriber-only and high-quality
                    streams.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>
    </div>
  );
}
