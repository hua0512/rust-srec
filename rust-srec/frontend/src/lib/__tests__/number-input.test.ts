import { describe, expect, it, vi } from 'vitest';

import { onNumberInputChange } from '../number-input';

/** Stands in for the change event a number input dispatches. */
function changeEvent(value: string) {
  const input = document.createElement('input');
  input.type = 'number';
  input.value = value;
  return { target: input } as unknown as React.ChangeEvent<HTMLInputElement>;
}

describe('onNumberInputChange', () => {
  it('reports the parsed number', () => {
    const onChange = vi.fn();

    onNumberInputChange(onChange)(changeEvent('42'));

    expect(onChange).toHaveBeenCalledWith(42);
  });

  it('keeps decimals', () => {
    const onChange = vi.fn();

    onNumberInputChange(onChange)(changeEvent('59.94'));

    expect(onChange).toHaveBeenCalledWith(59.94);
  });

  it('keeps zero rather than treating it as unset', () => {
    const onChange = vi.fn();

    onNumberInputChange(onChange)(changeEvent('0'));

    expect(onChange).toHaveBeenCalledWith(0);
  });

  it('reports undefined for an empty box instead of NaN', () => {
    const onChange = vi.fn();

    onNumberInputChange(onChange)(changeEvent(''));

    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  it('reports undefined when the box holds something unparseable', () => {
    const onChange = vi.fn();

    // A number input refuses to keep text, so the DOM value comes back empty.
    onNumberInputChange(onChange)(changeEvent('not a number'));

    expect(onChange).toHaveBeenCalledWith(undefined);
  });
});
