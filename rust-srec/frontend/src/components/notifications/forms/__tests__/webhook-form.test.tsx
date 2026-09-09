import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { WebhookForm } from '../webhook-form';

type Values = { settings: Record<string, unknown> };

function renderForm(headers: Array<[string, string]>) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      defaultValues: {
        settings: {
          url: 'https://example.invalid/hook',
          method: 'POST',
          headers,
          timeout_secs: 30,
          min_priority: 2,
          locale: 'en',
          enabled: true,
          auth: { type: 'None' },
        },
      },
    });
    return (
      <Form {...form}>
        <WebhookForm />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return { stored: () => form.getValues('settings') };
}

const keyInputs = () =>
  screen.queryAllByPlaceholderText('Key') as HTMLInputElement[];
const removeRow = (index: number) =>
  fireEvent.click(
    within(keyInputs()[index].closest('div.group')!).getAllByRole('button')[0],
  );

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe('WebhookForm custom headers', () => {
  it('lists the configured headers', () => {
    renderForm([
      ['X-Token', 'abc'],
      ['X-Trace', 'def'],
    ]);

    expect(keyInputs().map((input) => input.value)).toEqual([
      'X-Token',
      'X-Trace',
    ]);
  });

  // Rows keyed by position would leave the survivor rendered on the deleted
  // row's element, taking the caret and focus ring with it.
  it('removes the row that was asked for and leaves the other one in place', () => {
    const { stored } = renderForm([
      ['X-Token', 'abc'],
      ['X-Trace', 'def'],
    ]);
    const second = keyInputs()[1];

    removeRow(0);

    expect(keyInputs()).toEqual([second]);
    expect(stored().headers).toEqual([['X-Trace', 'def']]);
  });

  it('leaves the timeout unset when it is cleared', () => {
    const { stored } = renderForm([]);
    const timeout = screen.getByPlaceholderText('30') as HTMLInputElement;

    fireEvent.change(timeout, { target: { value: '' } });

    expect(timeout.value).toBe('');
    expect(stored().timeout_secs).toBeUndefined();
  });
});
