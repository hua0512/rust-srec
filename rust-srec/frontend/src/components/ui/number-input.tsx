import * as React from 'react';
import type { LucideIcon } from 'lucide-react';
import { useFormContext, useWatch } from 'react-hook-form';
import type { ControllerRenderProps } from 'react-hook-form';

import { Input } from './input';
import { IconInput } from './icon-input';
import { onNumberInputChange } from '@/lib/number-input';

interface NumberInputProps extends Omit<
  React.ComponentProps<'input'>,
  'value' | 'onChange' | 'onBlur' | 'type' | 'name' | 'ref'
> {
  /** The `field` handed to the surrounding `FormField` render callback. */
  field: ControllerRenderProps<any, any>;
  /** Renders the field with a leading icon, matching `IconInput`. */
  icon?: LucideIcon;
  /**
   * Replaces `field.onChange`, for a field that spells "unset" some other way
   * than an absent key — a nullable one that has to store `null` so the value
   * overrides an inherited layer instead of falling through to it.
   */
  onChange?: (value: number | undefined) => void;
}

/**
 * Numeric form field that can be emptied.
 *
 * The displayed value is read from the form rather than taken from `field`: a
 * controller answers a request for an absent value with whatever the field held
 * when it was created, so an emptied box would immediately fill itself back in
 * with the value the user just deleted.
 *
 * Clearing stores `undefined`, which leaves an optional field unset and lets a
 * field with a default fall back to it, instead of the `NaN` an empty numeric
 * input reports. Pass `onChange` to store something else for an empty box.
 */
export function NumberInput({
  field,
  icon,
  onChange,
  ...props
}: NumberInputProps) {
  const { control } = useFormContext();
  const value = useWatch({ control, name: field.name }) as
    | number
    | null
    | undefined;

  const inputProps = {
    disabled: field.disabled,
    ...props,
    type: 'number' as const,
    name: field.name,
    ref: field.ref,
    value: value ?? '',
    onChange: onNumberInputChange(onChange ?? field.onChange),
    onBlur: field.onBlur,
  };

  return icon ? (
    <IconInput icon={icon} {...inputProps} />
  ) : (
    <Input {...inputProps} />
  );
}
