import { fireEvent, screen, within } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { TimeBasedFilterForm } from '../TimeBasedFilterForm';
import { renderWithProviders } from '@/test/render';

type Values = { config: Record<string, unknown> };

function renderForm(config?: Record<string, unknown>) {
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({
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

  renderWithProviders(<Harness />);

  return { values: () => form.getValues('config') as Record<string, unknown> };
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

const timeline = () => screen.getByRole('group', { name: '24-hour timeline' });
const hour = (value: number) =>
  within(timeline()).getByRole('button', { name: `Hour ${value}` });

describe('time based filter form', () => {
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
