import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { NetworkSettingsCard } from '../network-settings-card';

function Harness() {
  const form = useForm<{ download_retry_policy: string | null }>({
    defaultValues: { download_retry_policy: null },
  });
  return (
    <Form {...form}>
      <NetworkSettingsCard
        form={form}
        paths={{ retryPolicy: 'download_retry_policy' }}
        configMode="json"
      />
    </Form>
  );
}

describe('NetworkSettingsCard', () => {
  it('points a template to its platform overrides for account choice', () => {
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
          <Harness />
        </I18nProvider>
      </QueryClientProvider>,
    );
    expect(
      screen.getByText(/Accounts are chosen per platform/),
    ).toBeInTheDocument();
  });
});
