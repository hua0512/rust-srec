import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Clapperboard } from 'lucide-react';
import { Template } from '@/api/schemas';
import {
  CONFIG_DESCRIPTION,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';

interface StreamerRecordingFieldsProps {
  form: UseFormReturn<any>;
  templates?: Template[];
}

/**
 * How a streamer is recorded: which template it inherits, its scheduling priority, and whether
 * monitoring is on. Paired with [`StreamerIdentityFields`] on the General tab.
 */
export function StreamerRecordingFields({
  form,
  templates,
}: StreamerRecordingFieldsProps) {
  const { i18n } = useLingui();

  return (
    <section className="space-y-4">
      <ConfigSectionHeading icon={Clapperboard}>
        <Trans>Recording</Trans>
      </ConfigSectionHeading>

      <div className="grid gap-4 md:grid-cols-2">
        <FormField
          control={form.control}
          name="template_id"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Template</Trans>
              </ConfigFieldLabel>
              <Select
                onValueChange={(val) =>
                  field.onChange(val === 'none' ? null : val)
                }
                value={field.value ? String(field.value) : 'none'}
              >
                <FormControl>
                  <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                    <SelectValue placeholder={i18n._(msg`Select template`)} />
                  </SelectTrigger>
                </FormControl>
                <SelectContent className={CONFIG_SELECT_CONTENT}>
                  <SelectItem value="none">
                    <Trans>None (Default)</Trans>
                  </SelectItem>
                  {templates?.map((template) => (
                    <SelectItem key={template.id} value={String(template.id)}>
                      {template.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Apply template settings.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="priority"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Priority</Trans>
              </ConfigFieldLabel>
              <Select onValueChange={field.onChange} value={field.value}>
                <FormControl>
                  <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                    <SelectValue placeholder={i18n._(msg`Select priority`)} />
                  </SelectTrigger>
                </FormControl>
                <SelectContent className={CONFIG_SELECT_CONTENT}>
                  <SelectItem value="HIGH">
                    <Trans>High</Trans>
                  </SelectItem>
                  <SelectItem value="NORMAL">
                    <Trans>Normal</Trans>
                  </SelectItem>
                  <SelectItem value="LOW">
                    <Trans>Low</Trans>
                  </SelectItem>
                </SelectContent>
              </Select>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Order this streamer is checked in.</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <FormField
        control={form.control}
        name="enabled"
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between gap-4 rounded-xl border border-border/50 bg-background/50 px-4 py-3 shadow-sm">
            <div className="space-y-1">
              <FormLabel className="cursor-pointer text-xs font-bold uppercase tracking-wider text-muted-foreground">
                <Trans>Enable monitoring</Trans>
              </FormLabel>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Check this streamer and record when it goes live.</Trans>
              </FormDescription>
            </div>
            <FormControl>
              <Switch checked={field.value} onCheckedChange={field.onChange} />
            </FormControl>
          </FormItem>
        )}
      />
    </section>
  );
}
