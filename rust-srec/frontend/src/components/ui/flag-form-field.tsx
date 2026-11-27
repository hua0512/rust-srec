import React, { useId } from 'react';
import {
  get,
  useFormContext,
  type Control,
  type FieldValues,
} from 'react-hook-form';
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
  control?: Control<FieldValues>;
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
  disabled?: boolean;
  showExperimentalBadge?: boolean;
  showPremiumBadge?: boolean;
  className?: string;
  children?: React.ReactNode;
};

type FlagRowProps = Pick<
  FlagFormFieldProps,
  | 'title'
  | 'description'
  | 'info'
  | 'showExperimentalBadge'
  | 'showPremiumBadge'
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
  control,
  fieldName,
  ariaLabel,
  inherited = false,
  checked,
  disabled = false,
  ...row
}: FlagFormFieldProps) {
  if (inherited) {
    return (
      <InheritedFlagField
        control={control}
        name={fieldName}
        ariaLabel={ariaLabel}
        disabled={disabled}
        {...row}
      />
    );
  }
  return (
    <FormField
      control={control}
      name={fieldName}
      render={({ field }) => {
        const isDisabled = disabled || Boolean(field.disabled);
        return (
          <FlagRow
            {...row}
            control={
              <Switch
                className="shrink-0"
                checked={checked ? checked(field.value) : Boolean(field.value)}
                onCheckedChange={field.onChange}
                disabled={isDisabled}
                aria-label={ariaLabel}
              />
            }
          />
        );
      }}
    />
  );
}

function FlagRow({
  title,
  description,
  info,
  showExperimentalBadge,
  showPremiumBadge,
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
            {showPremiumBadge && (
              <Badge
                variant="secondary"
                className="border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400"
              >
                <Trans>Premium</Trans>
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
  control: formControl,
  name,
  ariaLabel,
  disabled = false,
  ...row
}: Omit<FlagRowProps, 'labelId' | 'wrap' | 'control'> &
  Pick<FlagFormFieldProps, 'control' | 'ariaLabel' | 'disabled'> & {
    name: string;
  }) {
  const { i18n } = useLingui();
  const labelId = useId();
  const { formState } = useFormContext();
  return (
    <FormField
      // A controller shows the value it mounted with whenever the field is undefined, as it is
      // after a reset to saved values that leave the option out. Remounting on a new saved value
      // makes that reset show Inherit rather than the previous choice.
      key={JSON.stringify(get(formState.defaultValues, name)) ?? 'unset'}
      control={formControl}
      name={name}
      render={({ field }) => {
        const isDisabled = disabled || Boolean(field.disabled);
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
                aria-disabled={isDisabled || undefined}
                className={cn(
                  'inline-flex shrink-0 rounded-lg border border-border/50 bg-muted/30 p-0.5',
                  isDisabled && 'opacity-50',
                )}
              >
                {INHERITED_FLAG_CHOICES.map((choice) => (
                  <label
                    key={choice.id}
                    // The transparent outline shows only in forced colors, where the background
                    // that marks the selected choice is not drawn.
                    className={cn(
                      'rounded-md px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors has-checked:bg-background has-checked:text-foreground has-checked:shadow-sm has-checked:outline has-checked:outline-transparent has-focus-visible:ring-2 has-focus-visible:ring-ring',
                      isDisabled
                        ? 'cursor-not-allowed'
                        : 'cursor-pointer hover:text-foreground',
                    )}
                  >
                    <input
                      type="radio"
                      className="sr-only"
                      name={name}
                      value={choice.id}
                      checked={selected === choice.value}
                      disabled={isDisabled}
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
