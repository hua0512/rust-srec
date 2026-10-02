import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { NetworkSettingsCard } from '../network-settings-card';

vi.mock('@/server/functions', () => ({
  getStreamerCredentialSource: vi.fn(),
  getPlatformCredentialSource: vi.fn(),
  getTemplateCredentialSource: vi.fn(),
  refreshStreamerCredentials: vi.fn(),
  refreshPlatformCredentials: vi.fn(),
  refreshTemplateCredentials: vi.fn(),
}));

interface Values {
  cookies: string | null;
  credential_selection?: unknown;
  download_retry_policy: string | null;
}

function renderCard(credentialSelection?: unknown) {
  function Harness() {
    const form = useForm<Values>({
      defaultValues: {
        cookies: 'sid=legacy',
        credential_selection: credentialSelection,
        download_retry_policy: null,
      },
    });
    return (
      <Form {...form}>
        <NetworkSettingsCard
          form={form}
          paths={{
            cookies: 'cookies',
            credentialSelection: 'credential_selection',
            retryPolicy: 'download_retry_policy',
          }}
          configMode="json"
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

describe('NetworkSettingsCard', () => {
  // A template or streamer that has not been saved yet has no credential scope.
  // It cannot own profiles, but its legacy cookies must stay editable.
  it('keeps legacy cookies editable before the scope is saved', () => {
    renderCard();

    expect(screen.getByLabelText('Cookies')).toHaveValue('sid=legacy');
  });

  it('hides legacy cookies once a managed selection applies', () => {
    renderCard({ mode: 'none' });

    expect(screen.queryByLabelText('Cookies')).not.toBeInTheDocument();
  });
});
