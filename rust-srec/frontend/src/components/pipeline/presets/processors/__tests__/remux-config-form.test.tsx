import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, render } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { RemuxConfigForm } from '../remux-config-form';

it('keeps a saved NVENC preset when an editor loads it into an empty form', () => {
  let form!: UseFormReturn<Record<string, unknown>>;
  function Harness() {
    // Preset and step editors mount the form empty and reset it once the
    // saved config arrives.
    form = useForm<Record<string, unknown>>({ defaultValues: {} });
    return (
      <Form {...form}>
        <RemuxConfigForm control={form.control as never} />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );

  act(() =>
    form.reset({ video_codec: 'hevcnvenc', hwaccel: 'cuda', preset: 'p7' }),
  );

  expect(form.getValues('preset')).toBe('p7');
});
