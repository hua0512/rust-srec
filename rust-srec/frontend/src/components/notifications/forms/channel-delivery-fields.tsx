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
import { PRIORITY_NORMAL, priorityOptions } from '@/lib/priority';
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
                  <Trans>{opt.label}</Trans>
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
 * Both delivery fields stacked.
 *
 * The toggle sits on its own row rather than beside the priority select: it carries its label
 * inside its box, so in a shared row it has nothing to align its top edge against.
 */
export function ChannelDeliveryFields() {
  return (
    <div className="space-y-4">
      <MinPriorityField />
      <ChannelEnabledField />
    </div>
  );
}
