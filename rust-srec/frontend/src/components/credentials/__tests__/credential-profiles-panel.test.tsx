import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { CredentialProfilesPanel } from '../credential-profiles-panel';
import {
  createCredentialProfile,
  updateCredentialProfile,
  listCredentialProfiles,
  getEffectiveCredentialSelection,
} from '@/server/functions/credential-profiles';

vi.mock('@/server/functions/credential-profiles', () => ({
  createCredentialProfile: vi.fn(),
  updateCredentialProfile: vi.fn(),
  deleteCredentialProfile: vi.fn(),
  listCredentialProfiles: vi.fn(),
  getEffectiveCredentialSelection: vi.fn(),
  refreshCredentialProfile: vi.fn(),
  validateCredentialProfile: vi.fn(),
  previewCredentialConversion: vi.fn(),
  convertLegacyCredentials: vi.fn(),
  generateCredentialLogin: vi.fn(),
  pollCredentialLogin: vi.fn(),
}));

const profile = {
  id: 'account-a',
  platform_config_id: 'platform-a',
  owner: { type: 'platform' as const, platform_id: 'platform-a' },
  label: 'Account A',
  enabled: true,
  revision: 1,
  version: 2,
  has_cookies: true,
  has_refresh_token: true,
  has_access_token: false,
  has_reauth: false,
};
const onSelectionChange = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    {
      profile,
      health: null,
      references: [],
      capabilities: { validate: false, refresh: false, qr_login: false },
    },
  ]);
  vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
    configured: { mode: 'fixed', credential_id: profile.id },
    resolved: null,
    candidates: [],
    unavailable_reason: null,
  });
  vi.mocked(updateCredentialProfile).mockResolvedValue(profile);
  vi.mocked(createCredentialProfile).mockResolvedValue({
    ...profile,
    id: 'account-b',
  });
});

async function show(platformName = 'Bilibili') {
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
        <CredentialProfilesPanel
          owner={profile.owner}
          platformId="platform-a"
          platformName={platformName}
          selection={{ mode: 'fixed', credential_id: profile.id }}
          onSelectionChange={onSelectionChange}
          onConverted={vi.fn()}
        />
      </I18nProvider>
    </QueryClientProvider>,
  );
  await screen.findByRole('button', { name: 'Edit label' });
}

it('supports an empty-cookie Twitch profile using only its own access token', async () => {
  await show('Twitch');
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  expect(screen.queryByLabelText('Refresh token')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Twitch account' },
  });
  fireEvent.change(screen.getByLabelText('Access token'), {
    target: { value: 'twitch-token' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save profile' }));
  await waitFor(() =>
    expect(createCredentialProfile).toHaveBeenCalledWith({
      data: {
        owner: profile.owner,
        platform_id: 'platform-a',
        label: 'Twitch account',
        enabled: true,
        material: {
          cookies: '',
          refresh_token: null,
          access_token: 'twitch-token',
          reauth_config: null,
        },
      },
    }),
  );
});

it('metadata saves never submit secret placeholders or replace the bundle', async () => {
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Edit label' }));
  expect(screen.queryByLabelText('Cookies')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Renamed' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save profile' }));
  await waitFor(() =>
    expect(updateCredentialProfile).toHaveBeenCalledWith({
      data: {
        id: 'account-a',
        expected_version: 2,
        label: 'Renamed',
        enabled: true,
      },
    }),
  );
  expect(onSelectionChange).not.toHaveBeenCalled();
});

it('replacement opens empty and a second profile does not change the fixed selection', async () => {
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  expect(screen.getByLabelText('Cookies')).toHaveValue('');
  expect(screen.getByLabelText('Refresh token')).toHaveValue('');
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Account B' },
  });
  fireEvent.change(screen.getByLabelText('Cookies'), {
    target: { value: 'sid=b' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save profile' }));
  await waitFor(() =>
    expect(createCredentialProfile).toHaveBeenCalledWith({
      data: {
        owner: profile.owner,
        platform_id: 'platform-a',
        label: 'Account B',
        enabled: true,
        material: {
          cookies: 'sid=b',
          refresh_token: null,
          access_token: null,
          reauth_config: null,
        },
      },
    }),
  );
  expect(onSelectionChange).not.toHaveBeenCalled();
});

it('disabled profiles cannot start validation or refresh', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    {
      profile: { ...profile, enabled: false },
      health: null,
      references: [],
      capabilities: { validate: true, refresh: true, qr_login: false },
    },
  ]);
  await show();
  expect(screen.getByRole('button', { name: 'Validate' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Refresh' })).toBeDisabled();
});

it('explains which action recovers an unavailable selection', async () => {
  vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
    configured: { mode: 'fixed', credential_id: profile.id },
    resolved: null,
    candidates: [],
    unavailable_reason: 'login_required',
  });
  await show();
  expect(await screen.findByRole('status')).toHaveTextContent(
    'Every usable account needs a new login.',
  );
});
