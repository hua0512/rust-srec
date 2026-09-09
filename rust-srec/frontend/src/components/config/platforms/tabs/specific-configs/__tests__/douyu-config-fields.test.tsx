import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { PlatformSpecificTab } from '../../platform-specific-tab';

type Values = { platform_specific_config: Record<string, unknown> };

function renderFields(
  options?: Record<string, unknown>,
  { inherited = false } = {},
) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      defaultValues: { platform_specific_config: options },
    });
    return (
      <Form {...form}>
        <PlatformSpecificTab
          form={form}
          platformName="douyu"
          inherited={inherited}
        />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return {
    stored: () => form.getValues('platform_specific_config'),
  };
}

const cdnInput = () =>
  screen.getByLabelText('Preferred CDN') as HTMLInputElement;
const retriesInput = () =>
  screen.getByLabelText('API Request Retries') as HTMLInputElement;

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe('Douyu configuration', () => {
  it('leaves the fields empty when nothing is set and names the fallback', () => {
    renderFields();

    expect(cdnInput().value).toBe('');
    expect(cdnInput().placeholder).toBe('Default: ws-h5');
    expect(retriesInput().value).toBe('');
    expect(retriesInput().placeholder).toBe('Default: 3');
  });

  it('offers inheritance instead of a default on an overriding layer', () => {
    renderFields(undefined, { inherited: true });

    expect(cdnInput().placeholder).toBe('Inherited');
    expect(retriesInput().placeholder).toBe('Inherited');
  });

  it('shows the saved values', () => {
    renderFields({ cdn: 'hw-h5', request_retries: 5 });

    expect(cdnInput().value).toBe('hw-h5');
    expect(retriesInput().value).toBe('5');
  });

  it('can be emptied again', () => {
    const { stored } = renderFields({ cdn: 'hw-h5', request_retries: 5 });

    fireEvent.change(cdnInput(), { target: { value: '' } });
    fireEvent.change(retriesInput(), { target: { value: '' } });

    expect(cdnInput().value).toBe('');
    expect(retriesInput().value).toBe('');
    // Null rather than an absent key, so the cleared value still overrides a
    // legacy flat template setting instead of falling through to it.
    expect(stored().cdn).toBeNull();
    expect(stored().request_retries).toBeNull();
  });

  it('keeps a retry count of zero', () => {
    const { stored } = renderFields({ request_retries: 5 });

    fireEvent.change(retriesInput(), { target: { value: '0' } });

    expect(stored().request_retries).toBe(0);
  });
});
