import React, { useId } from 'react';
import { get, useFormContext } from 'react-hook-form';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from './form';
import { Badge } from './badge';
import { Switch } from './switch';
import { SETTING_ROW_CLASS } from './switch-card';
import { cn } from '@/lib/utils';

type FlagFormFieldProps = {
  controlPrefix?: string;
  fieldName: string;
  title?: string | React.ReactNode;
  description?: string | React.ReactNode;
  ariaLabel?: string;
  /**
   * Rendered beside the title. Takes a `FieldInfo` when the reason for the setting is longer
   * than the one line `description` has room for, so the row stays a row.
   */
  info?: React.ReactNode;
  /**
   * A layer that can leave the option to the layer below it, such as a template or streamer.
   * Shows Inherit, On and Off instead of a switch.
   */
  inherited?: boolean;
  /** The switch position for a stored value. Unused when `inherited`. */
  checked?: (value: unknown) => boolean;
  /** Called with the stored value; `null` only when `inherited`. */
  onChange?: (value: boolean | null) => void;
  showExperimentalBadge?: boolean;
  className?: string;
  children?: React.ReactNode;
};

type FlagRowProps = Pick<
  FlagFormFieldProps,
  | 'title'
  | 'description'
  | 'info'
  | 'showExperimentalBadge'
  | 'className'
  | 'children'
> & {
  labelId?: string;
  control: React.ReactElement;
};

export function FlagFormField({
  controlPrefix,
  fieldName,
  ariaLabel,
  inherited = false,
  checked,
  onChange,
  ...row
}: FlagFormFieldProps) {
  const name = controlPrefix ? `${controlPrefix}.${fieldName}` : fieldName;
  if (inherited) {
    return (
      <InheritedFlagField
        name={name}
        ariaLabel={ariaLabel}
        onChange={onChange}
        {...row}
      />
    );
  }
  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FlagRow
          {...row}
          control={
            <Switch
              className="shrink-0"
              checked={checked ? checked(field.value) : field.value}
              onCheckedChange={(value) => {
                field.onChange(value);
                onChange?.(value);
              }}
              aria-label={ariaLabel}
            />
          }
        />
      )}
    />
  );
}

function FlagRow({
  title,
  description,
  info,
  showExperimentalBadge,
  className,
  children,
  labelId,
  control,
}: FlagRowProps) {
  return (
    <FormItem className={cn(SETTING_ROW_CLASS, className)}>
      <div className="min-w-0 space-y-1">
        <FormLabel id={labelId}>
          <div className={'flex flex-row items-center gap-x-2'}>
            {title}
            {info}
            {showExperimentalBadge && (
              <Badge>
                <Trans>Experimental</Trans>
              </Badge>
            )}
          </div>
        </FormLabel>
        {description && (
          <FormDescription className="text-xs leading-relaxed">
            {description}
          </FormDescription>
        )}
        {children}
      </div>
      <FormControl>{control}</FormControl>
    </FormItem>
  );
}

const INHERITED_FLAG_CHOICES = [
  {
    id: 'inherit',
    value: null,
    label: msg({ message: 'Inherit', context: 'inherited flag' }),
  },
  {
    id: 'on',
    value: true,
    label: msg({ message: 'On', context: 'inherited flag' }),
  },
  {
    id: 'off',
    value: false,
    label: msg({ message: 'Off', context: 'inherited flag' }),
  },
] as const;

/**
 * Inherit sets the option to `null`, which every layer merge skips, so the layer below decides.
 * In a template it also overrides an older flat copy of the option stored beside the form's
 * values. Setting `undefined` instead would show the value the controller mounted with.
 * Anything but `true` or `false` shows as Inherit.
 */
function InheritedFlagField({
  name,
  ariaLabel,
  onChange,
  ...row
}: Omit<FlagRowProps, 'labelId' | 'control'> &
  Pick<FlagFormFieldProps, 'ariaLabel' | 'onChange'> & { name: string }) {
  const { i18n } = useLingui();
  const labelId = useId();
  const { formState } = useFormContext();
  return (
    <FormField
      // A controller shows the value it mounted with whenever the field is undefined, as it is
      // after a reset to saved values that leave the option out. Remounting on a new saved value
      // makes that reset show Inherit rather than the previous choice.
      key={JSON.stringify(get(formState.defaultValues, name)) ?? 'unset'}
      name={name}
      render={({ field }) => {
        const selected =
          field.value === true || field.value === false ? field.value : null;
        return (
          <FlagRow
            {...row}
            labelId={labelId}
            control={
              <div
                role="radiogroup"
                aria-label={ariaLabel}
                aria-labelledby={ariaLabel ? undefined : labelId}
                className="inline-flex shrink-0 rounded-lg border border-border/50 bg-muted/30 p-0.5"
              >
                {INHERITED_FLAG_CHOICES.map((choice) => (
                  <label
                    key={choice.id}
                    className="cursor-pointer rounded-md px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground has-checked:bg-background has-checked:text-foreground has-checked:shadow-sm has-focus-visible:ring-2 has-focus-visible:ring-ring"
                  >
                    <input
                      type="radio"
                      className="sr-only"
                      name={name}
                      value={choice.id}
                      checked={selected === choice.value}
                      onChange={() => {
                        field.onChange(choice.value);
                        onChange?.(choice.value);
                      }}
                    />
                    {i18n._(choice.label)}
                  </label>
                ))}
              </div>
            }
          />
        );
      }}
    />
  );
}
