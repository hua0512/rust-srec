import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Globe, KeyRound, Timer } from 'lucide-react';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { useFormContext } from 'react-hook-form';
import { IconInput } from '@/components/ui/icon-input';
import { SwitchCard } from '@/components/ui/switch-card';
import { priorityOptions } from '@/lib/priority';

export function GotifyForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
      <FormField
        control={form.control}
        name="settings.server_url"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Server URL</Trans>
            </FormLabel>
            <FormControl>
              <IconInput
                icon={Globe}
                placeholder={i18n._(msg`https://gotify.example.com`)}
                className="bg-background/50"
                {...field}
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name="settings.app_token"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>App Token</Trans>
            </FormLabel>
            <FormControl>
              <IconInput
                icon={KeyRound}
                type="password"
                placeholder={i18n._(msg`Gotify application token`)}
                className="bg-background/50"
                {...field}
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <div className="pt-2 grid grid-cols-3 gap-4">
        <FormField
          control={form.control}
          name="settings.min_priority"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Min Priority</Trans>
              </FormLabel>
              <Select
                onValueChange={(val) => field.onChange(Number(val))}
                defaultValue={String(field.value)}
              >
                <FormControl>
                  <SelectTrigger className="bg-background/50">
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  {priorityOptions().map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      <Trans>{opt.label}</Trans>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="settings.timeout_secs"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Timeout (s)</Trans>
              </FormLabel>
              <FormControl>
                <IconInput
                  icon={Timer}
                  type="number"
                  min={1}
                  max={300}
                  placeholder="30"
                  className="bg-background/50"
                  {...field}
                  onChange={(e) => field.onChange(e.target.valueAsNumber)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="settings.enabled"
          render={({ field }) => (
            <SwitchCard
              label={<Trans>Enabled</Trans>}
              checked={field.value}
              onCheckedChange={field.onChange}
              className="border-primary/10 bg-background/50 h-full"
            />
          )}
        />
      </div>
    </div>
  );
}
