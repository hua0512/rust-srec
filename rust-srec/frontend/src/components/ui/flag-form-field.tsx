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
  /**
   * Moves the control below the text when the row is too narrow for both, for a control
   * wider than a switch.
   */
  wrap?: boolean;
  control: React.ReactElement;
};

export function FlagFormField({
  fieldName,
  ariaLabel,
  inherited = false,
  checked,
  ...row
}: FlagFormFieldProps) {
  if (inherited) {
    return (
      <InheritedFlagField name={fieldName} ariaLabel={ariaLabel} {...row} />
    );
  }
  return (
    <FormField
      name={fieldName}
      render={({ field }) => (
        <FlagRow
          {...row}
          control={
            <Switch
              className="shrink-0"
              checked={checked ? checked(field.value) : field.value}
              onCheckedChange={field.onChange}
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
  wrap = false,
  control,
}: FlagRowProps) {
  return (
    <FormItem
      className={cn(SETTING_ROW_CLASS, wrap && 'flex-wrap gap-y-3', className)}
    >
      <div className={cn('min-w-0 space-y-1', wrap && 'grow basis-48')}>
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
  ...row
}: Omit<FlagRowProps, 'labelId' | 'wrap' | 'control'> &
  Pick<FlagFormFieldProps, 'ariaLabel'> & { name: string }) {
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
            wrap
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
                    // The transparent outline shows only in forced colors, where the background
                    // that marks the selected choice is not drawn.
                    className="cursor-pointer rounded-md px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground has-checked:bg-background has-checked:text-foreground has-checked:shadow-sm has-checked:outline has-checked:outline-transparent has-focus-visible:ring-2 has-focus-visible:ring-ring"
                  >
                    <input
                      type="radio"
                      className="sr-only"
                      name={name}
                      value={choice.id}
                      checked={selected === choice.value}
                      onChange={() => field.onChange(choice.value)}
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
