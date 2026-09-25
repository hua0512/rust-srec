import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import type { TemplateFormValues } from '../../template-editor';
import { PlatformOverrideCard } from '../platform-override-card';

function renderCard() {
  function Harness() {
    const form = useForm<TemplateFormValues>({
      defaultValues: { name: 'Test', platform_overrides: { huya: {} } },
    });
    return (
      <Form {...form}>
        <PlatformOverrideCard
          form={form}
          platformName="huya"
          onRemove={() => {}}
        />
      </Form>
    );
  }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
        <Harness />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe('PlatformOverrideCard', () => {
  // The resolver reads only the extractor options and the pipelines from a template's
  // per-platform override, so offering any other setting there would save it for nothing.
  it('offers only the settings a per-platform override applies', () => {
    renderCard();

    fireEvent.click(screen.getByText('huya'));

    expect(
      screen.getAllByRole('tab').map((tab) => tab.textContent?.trim()),
    ).toEqual(['Specific', 'Pipeline']);
  });
});
