import {
  FormControl,
  FormField,
  FormItem,
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
import {
  ChannelEnabledField,
  MinPriorityField,
  ChannelLocaleField,
} from './channel-delivery-fields';
import {
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

export function TelegramForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name="settings.bot_token"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Bot Token</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={KeyRound}
                type="password"
                placeholder={i18n._(msg`123456:ABC-DEF...`)}
                className={CONFIG_INPUT}
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
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Chat ID</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={Hash}
                placeholder={i18n._(msg`-1001234567890`)}
                className={CONFIG_INPUT}
                {...field}
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <div className="grid items-start gap-4 pt-2 sm:grid-cols-2">
        <FormField
          control={form.control}
          name="settings.parse_mode"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Parse Mode</Trans>
              </ConfigFieldLabel>
              <Select
                onValueChange={field.onChange}
                value={field.value ?? 'HTML'}
              >
                <FormControl>
                  <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                    <SelectValue placeholder={i18n._(msg`Select parse mode`)} />
                  </SelectTrigger>
                </FormControl>
                <SelectContent className={CONFIG_SELECT_CONTENT}>
                  <SelectItem value="HTML">HTML</SelectItem>
                  <SelectItem value="Markdown">Markdown</SelectItem>
                  <SelectItem value="MarkdownV2">MarkdownV2</SelectItem>
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
        <MinPriorityField />
      </div>
      <ChannelLocaleField />
      <ChannelEnabledField />
    </div>
  );
}
