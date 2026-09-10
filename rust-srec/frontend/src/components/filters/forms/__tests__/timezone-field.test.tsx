import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentType } from 'react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { CronFilterForm } from '../CronFilterForm';
import { TimeBasedFilterForm } from '../TimeBasedFilterForm';
import { TimezoneField } from '../TimezoneField';

type Values = { config: Record<string, unknown> };

function renderForm(Component: ComponentType, config: Values['config']) {
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({ defaultValues: { config } });
    return (
      <Form {...form}>
        <Component />
      </Form>
    );
  }

  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return { values: () => form.getValues('config') };
}

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

const timezoneInput = () => screen.getByRole('textbox', { name: 'Timezone' });

describe.each([
  {
    name: 'time based',
    Component: TimeBasedFilterForm,
    config: {
      days_of_week: ['Monday'],
      start_time: '19:15:00',
      end_time: '02:30:00',
    },
  },
  {
    name: 'cron',
    Component: CronFilterForm,
    config: { expression: '0 15 19 * * MON' },
  },
])('$name timezone', ({ Component, config }) => {
  it.each(['local', 'America/New_York'])(
    'preserves the existing %s value on focus and blur',
    (timezone) => {
      const initial = { ...config, timezone };
      const { values } = renderForm(Component, initial);
      const input = timezoneInput();

      expect(input).toHaveValue(
        timezone === 'local' ? 'Server local' : timezone,
      );
      fireEvent.focus(input);
      fireEvent.blur(input);

      expect(values()).toEqual(initial);
      expect(input).toHaveValue(
        timezone === 'local' ? 'Server local' : timezone,
      );
    },
  );

  it.each([undefined, null])(
    'displays an absent %s timezone as UTC',
    (timezone) => {
      const { values } = renderForm(Component, { ...config, timezone });

      expect(timezoneInput()).toHaveValue('UTC');
      expect(values().timezone).toBe(timezone);
    },
  );

  it('accepts a typed IANA name without changing clock values', () => {
    const { values } = renderForm(Component, { ...config, timezone: 'local' });
    const input = timezoneInput();

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: '  Asia/Kolkata  ' } });
    fireEvent.blur(input);

    expect(input).toHaveValue('Asia/Kolkata');
    expect(values()).toEqual({ ...config, timezone: 'Asia/Kolkata' });
  });

  it('preserves local when whitespace is added to its display label', () => {
    const initial = { ...config, timezone: 'local' };
    const { values } = renderForm(Component, initial);
    const input = timezoneInput();

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: ' Server local ' } });
    fireEvent.blur(input);

    expect(input).toHaveValue('Server local');
    expect(values()).toEqual(initial);
  });

  it.each(['', '   '])('clearing to %j writes explicit UTC', (value) => {
    const { values } = renderForm(Component, {
      ...config,
      timezone: 'America/New_York',
    });
    const input = timezoneInput();

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value } });
    // A submit before blur must still send UTC rather than an empty zone.
    expect(values()).toEqual({ ...config, timezone: 'UTC' });
    fireEvent.blur(input);
    expect(input).toHaveValue('UTC');
  });
});

it('offers UTC and the backend server local timezone with distinct wire values', async () => {
  const { values } = renderForm(TimezoneField, { timezone: 'UTC' });
  expect(timezoneInput()).toHaveAccessibleDescription(
    /Server local uses the backend server's timezone/,
  );

  fireEvent.click(screen.getByRole('button', { name: 'Choose timezone' }));
  fireEvent.click(await screen.findByRole('option', { name: 'Server local' }));
  await waitFor(() => expect(timezoneInput()).toHaveValue('Server local'));
  expect(values().timezone).toBe('local');

  fireEvent.click(screen.getByRole('button', { name: 'Choose timezone' }));
  fireEvent.click(await screen.findByRole('option', { name: 'UTC' }));
  await waitFor(() => expect(timezoneInput()).toHaveValue('UTC'));
  expect(values().timezone).toBe('UTC');
});
