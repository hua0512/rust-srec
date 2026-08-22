import { useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { SwitchCard } from '@/components/ui/switch-card';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { PRIORITY_NORMAL, priorityOptions } from '@/lib/priority';
import { locales, localeNativeNames } from '@/integrations/lingui/i18n';

/**
 * Derived from the interface locales rather than listed again here.
 *
 * The backend renders notifications from its own YAML under `rust-srec/locales/`, so the two
 * sets have to stay in step. A locale offered here with no file there degrades to English rather
 * than failing, which is why this can follow the interface list rather than fetch its own.
 */
const NOTIFICATION_LOCALE_OPTIONS = locales.map((locale) => ({
  value: locale,
  label: localeNativeNames[locale],
}));

/**
 * Stands in for "no override" inside the select.
 *
 * Radix rejects an empty `SelectItem` value, reserving it for "nothing selected", so the choice
 * that means "follow the server" needs a value of its own. It is mapped back to the empty string
 * the schema and backend expect on change.
 */
const FOLLOW_SERVER_LOCALE = '__server__';
import {
  CONFIG_DESCRIPTION,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

/**
 * Delivery controls common to every notification channel.
 *
 * Exported as individual fields rather than one block because the channel forms lay them out
 * differently — some pair them two-up, others slot the priority select into a three-column row
 * beside a channel-specific field. Sharing the fields rather than the layout is what keeps their
 * labels and options from drifting, as "Min Priority" vs "Minimum Priority" previously had.
 */
export function MinPriorityField({
  description = true,
}: {
  description?: boolean;
}) {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <FormField
      control={form.control}
      name="settings.min_priority"
      render={({ field }) => (
        <FormItem className="space-y-2">
          <ConfigFieldLabel>
            <Trans>Minimum Priority</Trans>
          </ConfigFieldLabel>
          {/* Controlled, not `defaultValue`: the edit form is populated by `reset()` after
              mount, and an uncontrolled Select would never pick the loaded value up. */}
          {/* Falls back rather than rendering blank: a channel saved without an explicit
              priority should still show the level it will actually be treated as. */}
          <Select
            onValueChange={(val) => field.onChange(Number(val))}
            value={String(field.value ?? PRIORITY_NORMAL)}
          >
            <FormControl>
              <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                <SelectValue />
              </SelectTrigger>
            </FormControl>
            <SelectContent className={CONFIG_SELECT_CONTENT}>
              {priorityOptions().map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {i18n._(opt.label)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {description && (
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>Filter events below this priority</Trans>
            </FormDescription>
          )}
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

/**
 * Language this channel's notifications are written in.
 *
 * Separate from the interface language: the person reading a Telegram chat or a shared alert
 * mailbox is not necessarily the person who set this page's language. Empty means the server's
 * own language, which is what the backend treats as no override.
 */
export function ChannelLocaleField({
  description = true,
}: {
  description?: boolean;
}) {
  const form = useFormContext();

  return (
    <FormField
      control={form.control}
      name="settings.locale"
      render={({ field }) => (
        <FormItem className="space-y-2">
          <ConfigFieldLabel>
            <Trans>Notification language</Trans>
          </ConfigFieldLabel>
          <Select
            onValueChange={(value) =>
              field.onChange(value === FOLLOW_SERVER_LOCALE ? '' : value)
            }
            value={field.value || FOLLOW_SERVER_LOCALE}
          >
            <FormControl>
              <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                <SelectValue />
              </SelectTrigger>
            </FormControl>
            <SelectContent className={CONFIG_SELECT_CONTENT}>
              {/* Radix reserves the empty string for "no selection", so the follow-the-server
                  choice needs a sentinel of its own; it is mapped back on change. */}
              <SelectItem value={FOLLOW_SERVER_LOCALE}>
                <Trans>Same as server</Trans>
              </SelectItem>
              {NOTIFICATION_LOCALE_OPTIONS.map((locale) => (
                <SelectItem key={locale.value} value={locale.value}>
                  {locale.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {description && (
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>
                The language this channel's messages are written in, whoever
                reads them.
              </Trans>
            </FormDescription>
          )}
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

export function ChannelEnabledField() {
  const form = useFormContext();

  return (
    <FormField
      control={form.control}
      name="settings.enabled"
      render={({ field }) => (
        <SwitchCard
          label={<Trans>Enabled</Trans>}
          checked={field.value}
          onCheckedChange={field.onChange}
          className="h-full"
        />
      )}
    />
  );
}

/**
 * The delivery fields stacked.
 *
 * The toggle sits on its own row rather than beside the selects: it carries its label inside its
 * box, so in a shared row it has nothing to align its top edge against.
 */
export function ChannelDeliveryFields() {
  return (
    <div className="space-y-4">
      <div className="grid gap-4 @md:grid-cols-2">
        <MinPriorityField />
        <ChannelLocaleField />
      </div>
      <ChannelEnabledField />
    </div>
  );
}
