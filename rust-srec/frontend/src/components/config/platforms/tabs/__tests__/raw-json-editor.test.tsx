import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { PlatformSpecificTab } from '../platform-specific-tab';

type Values = { platform_specific_config: unknown };

function renderEditor(initial?: Record<string, unknown>) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      defaultValues: { platform_specific_config: initial },
    });
    return (
      <Form {...form}>
        {/* A platform without a form view opens straight into the JSON editor. */}
        <PlatformSpecificTab form={form} platformName="unknown" />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  const textarea = screen.getByPlaceholderText(
    '{ ... }',
  ) as HTMLTextAreaElement;
  return {
    textarea,
    stored: () => form.getValues('platform_specific_config'),
    form: () => form,
  };
}

function type(textarea: HTMLTextAreaElement, value: string) {
  fireEvent.change(textarea, { target: { value } });
}

describe('platform raw JSON editor', () => {
  it('shows the saved options as indented JSON', () => {
    const { textarea } = renderEditor({ cdn: 'ws-h5' });

    expect(textarea.value).toBe('{\n  "cdn": "ws-h5"\n}');
  });

  it('leaves compact JSON exactly as typed', () => {
    const { textarea, stored } = renderEditor();

    type(textarea, '{"cdn":"hw-h5"}');

    expect(textarea.value).toBe('{"cdn":"hw-h5"}');
    expect(stored()).toEqual({ cdn: 'hw-h5' });
  });

  it('keeps the half-typed text and stores nothing while the JSON is broken', () => {
    const { textarea, stored } = renderEditor({ cdn: 'ws-h5' });

    type(textarea, '{"cdn":');

    expect(textarea.value).toBe('{"cdn":');
    expect(stored()).toEqual({ cdn: 'ws-h5' });
    expect(screen.getByText(/Invalid JSON/)).toBeInTheDocument();
  });

  it('recovers once the JSON is complete again', () => {
    const { textarea, stored } = renderEditor();

    type(textarea, '{"cdn":');
    type(textarea, '{"cdn":"hw-h5"}');

    expect(screen.queryByText(/Invalid JSON/)).not.toBeInTheDocument();
    expect(stored()).toEqual({ cdn: 'hw-h5' });
  });

  it('clears the options when the box is emptied', () => {
    const { textarea, stored } = renderEditor({ cdn: 'ws-h5' });

    type(textarea, '');

    expect(stored()).toBeNull();
  });

  it('adopts options loaded from outside the editor', () => {
    const { textarea, form } = renderEditor({ cdn: 'ws-h5' });

    type(textarea, '{"cdn":"hw-h5"}');
    act(() => form().reset({ platform_specific_config: { cdn: 'tct-h5' } }));

    expect(textarea.value).toBe('{\n  "cdn": "tct-h5"\n}');
  });
});
