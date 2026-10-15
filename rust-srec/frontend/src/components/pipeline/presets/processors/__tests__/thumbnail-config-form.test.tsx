import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { ThumbnailConfigForm } from '../thumbnail-config-form';

vi.mock('motion/react', () => ({
  motion: { div: ({ children }: { children: unknown }) => children },
}));

function renderForm(defaultValues: object) {
  function Harness() {
    const form = useForm<any>({ defaultValues });
    return (
      <Form {...form}>
        <ThumbnailConfigForm control={form.control} />
      </Form>
    );
  }
  return render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
}

describe('ThumbnailConfigForm', () => {
  it('disables the width while native resolution is on', () => {
    renderForm({ preserve_resolution: true, width: 320 });
    const nativeSwitch = screen.getByRole('switch', {
      name: 'Native resolution',
    });
    const width = screen.getByPlaceholderText('320');

    expect(nativeSwitch).toBeChecked();
    expect(width).toBeDisabled();

    fireEvent.click(nativeSwitch);

    expect(nativeSwitch).not.toBeChecked();
    expect(width).toBeEnabled();
  });
});
