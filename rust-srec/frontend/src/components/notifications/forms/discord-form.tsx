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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { useFormContext } from 'react-hook-form';
import { Switch } from '@/components/ui/switch';

export function DiscordForm() {
  const form = useFormContext();

  return (
    <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
      <FormField
        control={form.control}
        name="settings.webhook_url"
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
          name="settings.username"
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
          name="settings.avatar_url"
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
      <div className="pt-2 grid grid-cols-2 gap-4">
        <FormField
          control={form.control}
          name="settings.min_priority"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Min Priority</Trans>
              </FormLabel>
              <Select onValueChange={field.onChange} defaultValue={field.value}>
                <FormControl>
                  <SelectTrigger className="bg-background/50">
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  <SelectItem value="Low">Low</SelectItem>
                  <SelectItem value="Normal">Normal</SelectItem>
                  <SelectItem value="High">High</SelectItem>
                  <SelectItem value="Critical">Critical</SelectItem>
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="settings.enabled"
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-primary/10 bg-background/50 p-3 shadow-sm h-full">
              <div className="space-y-0.5">
                <FormLabel>
                  <Trans>Enabled</Trans>
                </FormLabel>
              </div>
              <FormControl>
                <Switch
                  checked={field.value}
                  onCheckedChange={field.onChange}
                />
              </FormControl>
            </FormItem>
          )}
        />
      </div>
    </div>
  );
}
