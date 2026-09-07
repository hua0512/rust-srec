import type { ChangeEvent } from 'react';

/**
 * Adapts an `<input type="number">` change event to a numeric form field.
 *
 * An empty or unparseable box reports `undefined` instead of the `NaN` that
 * `valueAsNumber` yields, which is what every caller's schema expects: an
 * optional field returns to unset, a field with a default falls back to it, and
 * a required one reports that it is missing rather than failing on a value that
 * is not a number.
 *
 * Pair it with `value={field.value ?? ''}` so the input stays controlled once
 * the field is cleared.
 *
 * ```tsx
 * <Input
 *   type="number"
 *   value={field.value ?? ''}
 *   onChange={onNumberInputChange(field.onChange)}
 * />
 * ```
 */
export function onNumberInputChange(
  onChange: (value: number | undefined) => void,
) {
  return (event: ChangeEvent<HTMLInputElement>) => {
    const value = event.target.valueAsNumber;
    onChange(Number.isNaN(value) ? undefined : value);
  };
}
