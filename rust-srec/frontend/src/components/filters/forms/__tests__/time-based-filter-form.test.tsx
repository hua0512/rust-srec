import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { TimeBasedFilterForm } from '../TimeBasedFilterForm';

function renderForm(config?: Record<string, unknown>) {
  let form!: UseFormReturn<any>;

  function Harness() {
    form = useForm<any>({
      defaultValues: {
        config: {
          days_of_week: [],
          start_time: '',
          end_time: '',
          ...config,
        },
      },
    });
    return (
      <Form {...form}>
        <TimeBasedFilterForm />
      </Form>
    );
  }

  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );

  return { values: () => form.getValues('config') as Record<string, any> };
}

/**
 * jsdom does not run the browser's default keyboard activation, so a key press is modelled the
 * way a browser handles Space/Enter on a native control: the assertions pin the element to a
 * focusable `<button>`, and the resulting activation is dispatched as a click.
 */
function pressKey(element: HTMLElement) {
  expect(element.tagName).toBe('BUTTON');
  expect(element).not.toHaveAttribute('tabindex', '-1');
  fireEvent.click(element);
}

const day = (name: string) => screen.getByRole('button', { name });
const timeline = () => screen.getByRole('group', { name: '24-hour timeline' });
const hour = (value: number) =>
  within(timeline()).getByRole('button', { name: `Hour ${value}` });

describe('time based filter form', () => {
  it('names every day toggle and reports whether it is selected', () => {
    renderForm({ days_of_week: ['Monday'] });

    expect(day('Monday')).toHaveAttribute('aria-pressed', 'true');
    expect(day('Sunday')).toHaveAttribute('aria-pressed', 'false');
  });

  it('toggles a day from the keyboard', () => {
    const { values } = renderForm();

    pressKey(day('Wednesday'));

    expect(day('Wednesday')).toHaveAttribute('aria-pressed', 'true');
    expect(values().days_of_week).toEqual(['Wednesday']);

    pressKey(day('Wednesday'));

    expect(day('Wednesday')).toHaveAttribute('aria-pressed', 'false');
    expect(values().days_of_week).toEqual([]);
  });

  it('exposes the timeline as a labelled group of named hours', () => {
    renderForm({ start_time: '10:00:00', end_time: '12:00:00' });

    expect(within(timeline()).getAllByRole('button')).toHaveLength(24);
    expect(hour(11)).toHaveAttribute('aria-pressed', 'true');
    expect(hour(20)).toHaveAttribute('aria-pressed', 'false');
  });

  it('starts a new window when an hour is activated from the keyboard', () => {
    const { values } = renderForm({
      start_time: '10:00:00',
      end_time: '12:00:00',
    });

    pressKey(hour(20));

    expect(values().start_time).toBe('20:00:00');
    expect(values().end_time).toBe('');

    pressKey(hour(22));

    expect(values().end_time).toBe('22:00:00');
    expect(hour(21)).toHaveAttribute('aria-pressed', 'true');
  });
});
