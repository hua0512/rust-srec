import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { EngineOverrideCard } from '../engine-override-card';

type Values = { engines_override?: Record<string, unknown> };

function renderCard(onRemove: () => void) {
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({ defaultValues: {} });
    return (
      <Form {...form}>
        <EngineOverrideCard
          engineId="ffmpeg-1"
          engineName="FFmpeg"
          engineType="FFMPEG"
          onRemove={onRemove}
        />
      </Form>
    );
  }

  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
}

describe('engine override card', () => {
  it('names the icon-only remove control', () => {
    const onRemove = vi.fn();
    renderCard(onRemove);

    fireEvent.click(
      screen.getByRole('button', { name: 'Remove engine override' }),
    );

    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});
