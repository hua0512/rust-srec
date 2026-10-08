import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import type { ReactNode } from 'react';
import { FormProvider, useForm } from 'react-hook-form';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render } from '@testing-library/react';
import {
  getCredentialCapabilities,
  getEffectiveCredentialSelection,
  listCredentialProfiles,
} from '@/server/functions/credential-profiles';
import type {
  CredentialOwner,
  CredentialProfileDetail,
  PlatformCredentialCapabilities,
} from '@/api/schemas/credential-profiles';

export const profile = {
  id: 'account-a',
  label: 'Account A',
  enabled: true,
  version: 2,
};

export function accountDetail(
  overrides: Partial<CredentialProfileDetail> = {},
): CredentialProfileDetail {
  return {
    profile,
    health: null,
    references: { selections: [], recordings: [] },
    capabilities: { validate: false, refresh: false, qr_login: false },
    ...overrides,
  };
}

/** What the backend's providers report for the platforms the tests use. */
const PLATFORM_CAPABILITIES: Record<
  string,
  Partial<PlatformCredentialCapabilities>
> = {
  bilibili: {
    refresh_token: true,
    access_token: true,
    qr_login: true,
    check: true,
    refresh: true,
  },
  soop: { reauth_login: true, check: true, refresh: true },
  twitch: { access_token: true, token_only: true, check: true },
  streamlink: { per_streamer_selection: true },
};

/** Serves the capabilities of `platformName` for every platform ID. */
export function mockPlatformCapabilities(platformName: string) {
  vi.mocked(getCredentialCapabilities).mockResolvedValue({
    refresh_token: false,
    access_token: false,
    token_only: false,
    reauth_login: false,
    qr_login: false,
    check: false,
    refresh: false,
    per_streamer_selection: false,
    ...PLATFORM_CAPABILITIES[platformName.toLowerCase()],
  });
}

/**
 * Resets the mocked credential server functions: the platform has one account
 * and the scope's saved selection is that account, available.
 */
export function mockCredentialApi() {
  vi.clearAllMocks();
  mockPlatformCapabilities('cookies');
  vi.mocked(listCredentialProfiles).mockResolvedValue([accountDetail()]);
  vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
    configured: { mode: 'fixed', credential_id: profile.id },
    resolved: null,
    candidates: [],
    unavailable_reason: null,
  });
}

export const platformOwner: CredentialOwner = {
  type: 'platform',
  platform_id: 'platform-a',
};
export const templateOwner: CredentialOwner = {
  type: 'template',
  template_id: 'template-a',
};
export const streamerOwner: CredentialOwner = {
  type: 'streamer',
  streamer_id: 'streamer-a',
};

/** The account settings always render inside a configuration form. */
function InForm({ children }: { children: ReactNode }) {
  const form = useForm();
  return <FormProvider {...form}>{children}</FormProvider>;
}

export function newQueryClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

export function renderCredentials(
  ui: ReactNode,
  client: QueryClient = newQueryClient(),
) {
  render(
    <QueryClientProvider client={client}>
      <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
        <InForm>{ui}</InForm>
      </I18nProvider>
    </QueryClientProvider>,
  );
  return client;
}
