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
 * input reports.
 */
export function NumberInput({ field, icon, ...props }: NumberInputProps) {
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
    onChange: onNumberInputChange(field.onChange),
    onBlur: field.onBlur,
  };

  return icon ? (
    <IconInput icon={icon} {...inputProps} />
  ) : (
    <Input {...inputProps} />
  );
}
