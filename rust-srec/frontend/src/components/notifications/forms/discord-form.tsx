import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { Globe, User } from 'lucide-react';
import { useFormContext } from 'react-hook-form';

export function DiscordForm() {
  const form = useFormContext();

  return (
    <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
      <FormField
        control={form.control}
        name="discord_webhook_url"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Webhook URL</Trans>
            </FormLabel>
            <FormControl>
              <div className="relative">
                <Globe className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                <Input
                  placeholder="https://discord.com/api/webhooks/..."
                  {...field}
                  className="pl-9 bg-background/50"
                />
              </div>
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <div className="grid grid-cols-2 gap-4">
        <FormField
          control={form.control}
          name="discord_username"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Username (Optional)</Trans>
              </FormLabel>
              <FormControl>
                <div className="relative">
                  <User className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                  <Input
                    placeholder="Bot Name"
                    {...field}
                    className="pl-9 bg-background/50"
                  />
                </div>
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="discord_avatar_url"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Avatar URL (Optional)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  placeholder="https://..."
                  {...field}
                  className="bg-background/50"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  );
}
