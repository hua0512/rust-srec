import { act, fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form, FormControl, FormField, FormItem } from '@/components/ui/form';
import { NumberInput } from '../number-input';

function renderInput(initial?: number) {
  let form!: UseFormReturn<any>;
  function Harness() {
    form = useForm<any>({ defaultValues: { retries: initial } });
    return (
      <Form {...form}>
        <FormField
          control={form.control}
          name="retries"
          render={({ field }) => (
            <FormItem>
              <FormControl>
                <NumberInput field={field} placeholder="attempts" />
              </FormControl>
            </FormItem>
          )}
        />
      </Form>
    );
  }
  render(<Harness />);
  return {
    input: screen.getByPlaceholderText('attempts') as HTMLInputElement,
    stored: () => form.getValues('retries'),
    form: () => form,
  };
}

describe('NumberInput', () => {
  it('shows the stored value', () => {
    const { input } = renderInput(5);

    expect(input.value).toBe('5');
  });

  it('stores what was typed', () => {
    const { input, stored } = renderInput();

    fireEvent.change(input, { target: { value: '7' } });

    expect(stored()).toBe(7);
  });

  // The field's controller answers a request for an absent value with the value
  // the field started with, which would refill the box the moment it is cleared.
  it('stays empty after the value it started with is deleted', () => {
    const { input, stored } = renderInput(5);

    fireEvent.change(input, { target: { value: '' } });

    expect(input.value).toBe('');
    expect(stored()).toBeUndefined();
  });

  it('keeps a typed zero', () => {
    const { input, stored } = renderInput(5);

    fireEvent.change(input, { target: { value: '0' } });

    expect(input.value).toBe('0');
    expect(stored()).toBe(0);
  });

  it('follows a value loaded from outside', () => {
    const { input, form } = renderInput(5);

    fireEvent.change(input, { target: { value: '' } });
    act(() => form().reset({ retries: 9 }));

    expect(input.value).toBe('9');
  });
});
