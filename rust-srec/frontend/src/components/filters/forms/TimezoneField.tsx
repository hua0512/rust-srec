import { useFormContext } from 'react-hook-form';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';

import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { EditableCombobox } from '@/components/ui/editable-combobox';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';

type TimezoneValues = { config: { timezone?: string | null } };

export function TimezoneField() {
  const { control } = useFormContext<TimezoneValues>();
  const { i18n } = useLingui();
  const serverLocal = i18n._(msg`Server local`);

  return (
    <FormField
      control={control}
      name="config.timezone"
      render={({ field }) => {
        const timezone = field.value ?? 'UTC';

        return (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Timezone</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <EditableCombobox
                buttonLabel={i18n._(msg`Choose timezone`)}
                displayValue={timezone === 'local' ? serverLocal : timezone}
                selectedValue={timezone}
                placeholder="Europe/Berlin"
                onInputChange={(value) => {
                  const nextTimezone = value.trim();
                  field.onChange(
                    nextTimezone === serverLocal
                      ? 'local'
                      : nextTimezone || 'UTC',
                  );
                }}
                // Blur must not write the translated display label over 'local'.
                onInputBlur={() => field.onBlur()}
                onOptionSelect={(option) => {
                  field.onChange(option.value);
                  field.onBlur();
                }}
                options={[
                  {
                    value: 'UTC',
                    inputValue: 'UTC',
                    label: 'UTC',
                    searchValue: 'UTC',
                  },
                  {
                    value: 'local',
                    inputValue: serverLocal,
                    label: serverLocal,
                    searchValue: `local ${serverLocal}`,
                  },
                ]}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>
                Server local uses the backend server's timezone. You can also
                enter an IANA name such as Europe/Berlin. Clearing selects UTC.
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        );
      }}
    />
  );
}
