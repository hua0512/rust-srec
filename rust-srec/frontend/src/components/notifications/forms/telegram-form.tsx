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
import { KeyRound, Hash } from 'lucide-react';
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

export function TelegramForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
      <FormField
        control={form.control}
        name="settings.bot_token"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Bot Token</Trans>
            </FormLabel>
            <FormControl>
              <IconInput
                icon={KeyRound}
                type="password"
                placeholder={i18n._(msg`123456:ABC-DEF...`)}
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
        name="settings.chat_id"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Chat ID</Trans>
            </FormLabel>
            <FormControl>
              <IconInput
                icon={Hash}
                placeholder={i18n._(msg`-1001234567890`)}
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
          name="settings.parse_mode"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Parse Mode</Trans>
              </FormLabel>
              <Select onValueChange={field.onChange} defaultValue={field.value}>
                <FormControl>
                  <SelectTrigger className="bg-background/50">
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  <SelectItem value="HTML">HTML</SelectItem>
                  <SelectItem value="Markdown">Markdown</SelectItem>
                  <SelectItem value="MarkdownV2">MarkdownV2</SelectItem>
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
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
                  <SelectItem value="Low">
                    <Trans>Low</Trans>
                  </SelectItem>
                  <SelectItem value="Normal">
                    <Trans>Normal</Trans>
                  </SelectItem>
                  <SelectItem value="High">
                    <Trans>High</Trans>
                  </SelectItem>
                  <SelectItem value="Critical">
                    <Trans>Critical</Trans>
                  </SelectItem>
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
