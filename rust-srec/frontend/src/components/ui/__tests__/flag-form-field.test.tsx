import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { useForm } from 'react-hook-form';
import { describe, expect, it, vi } from 'vitest';

import { Form } from '@/components/ui/form';
import { renderWithProviders } from '@/test/render';
import { FlagFormField } from '../flag-form-field';

type Flag = boolean | null | undefined;
type Values = { options: { flag?: boolean | null } };

const values = (flag: Flag): Values => ({
  options: flag === undefined ? {} : { flag },
});

/** An option that is on unless stored `false`, as Bigo's original-stream recording is. */
function renderFlag(flag: Flag) {
  const onSave = vi.fn<(data: Values) => void>();
  let reset!: (next: Flag) => void;
  function Harness() {
    const form = useForm<Values>({ defaultValues: values(flag) });
    reset = (next) => form.reset(values(next));
    return (
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSave)}>
          <FlagFormField
            inherited
            fieldName="options.flag"
            title="Flag"
            checked={(value) => value !== false}
          />
          <button type="submit">Save</button>
        </form>
      </Form>
    );
  }
  renderWithProviders(<Harness />);

  const save = async () => {
    const calls = onSave.mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(calls + 1));
    return onSave.mock.calls[calls][0].options.flag;
  };
  return { save, reset: (next: Flag) => act(() => reset(next)) };
}

const choice = (name: string) => screen.getByRole('radio', { name });

describe('FlagFormField on a template or streamer', () => {
  it.each([undefined, null])('shows %j as Inherit', (flag) => {
    renderFlag(flag);

    expect(choice('Inherit')).toBeChecked();
    expect(choice('On')).not.toBeChecked();
    expect(choice('Off')).not.toBeChecked();
  });

  it('stores Off as false and Inherit as null', async () => {
    const { save } = renderFlag(true);

    fireEvent.click(choice('Off'));
    expect(await save()).toBe(false);
    fireEvent.click(choice('Inherit'));
    expect(await save()).toBeNull();
  });

  it('shows Inherit after a reset to saved values without the option', () => {
    const { reset } = renderFlag(true);
    expect(choice('On')).toBeChecked();

    reset(undefined);

    expect(choice('Inherit')).toBeChecked();
  });
});
