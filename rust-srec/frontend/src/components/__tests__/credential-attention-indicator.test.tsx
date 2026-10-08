import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { CredentialAttentionIndicator } from '../credential-attention-indicator';
import type { CredentialAttention } from '@/api/schemas/credential-profiles';
import { listCredentialAttention } from '@/server/functions/credential-profiles';

vi.mock('@tanstack/react-router', () => ({
  Link: ({
    children,
    to,
    params,
    className,
  }: {
    children: ReactNode;
    to: string;
    params?: { platformId: string };
    className?: string;
  }) => (
    <a
      href={to.replace('$platformId', params?.platformId ?? '')}
      className={className}
    >
      {children}
    </a>
  ),
}));

vi.mock('@/server/functions/credential-profiles', () => ({
  listCredentialAttention: vi.fn(),
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function account(
  id: string,
  platform: string,
  reason: 'login_required' | 'refresh_failing',
): CredentialAttention {
  return {
    profile: {
      id,
      label: `Account ${id}`,
      enabled: true,
      version: 1,
      platform_config_id: `platform-${platform}`,
    },
    platform_name: platform,
    reason,
    health: {
      validity: reason === 'login_required' ? 'invalid' : 'needs_refresh',
      last_check_at: reason === 'login_required' ? Date.now() - 60_000 : null,
      refresh_failure_count: reason === 'login_required' ? 0 : 3,
    },
  };
}

function renderIndicator(accounts: CredentialAttention[]) {
  vi.mocked(listCredentialAttention).mockResolvedValue(accounts);
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nProvider i18n={i18n}>
        <CredentialAttentionIndicator />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe('CredentialAttentionIndicator', () => {
  beforeEach(() => {
    vi.mocked(listCredentialAttention).mockReset();
  });

  it('renders nothing while no account needs attention', async () => {
    const { container } = renderIndicator([]);
    await waitFor(() => expect(listCredentialAttention).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('counts accounts needing login and links each to its platform', async () => {
    renderIndicator([
      account('a', 'bilibili', 'login_required'),
      account('b', 'twitch', 'login_required'),
    ]);

    fireEvent.click(
      await screen.findByRole('button', { name: '2 accounts need login' }),
    );

    const bilibili = screen.getByRole('region', { name: 'Bilibili' });
    expect(
      within(bilibili).getByText('Account a').closest('a'),
    ).toHaveAttribute('href', '/config/platforms/platform-bilibili');
    expect(within(bilibili).getByText('Needs a new login')).toBeInTheDocument();
    expect(within(bilibili).getByText(/^Last checked/)).toBeInTheDocument();
    const twitch = screen.getByRole('region', { name: 'Twitch' });
    expect(within(twitch).getByText('Account b').closest('a')).toHaveAttribute(
      'href',
      '/config/platforms/platform-twitch',
    );
  });

  it('says attention when some accounts only keep failing to refresh', async () => {
    renderIndicator([
      account('a', 'bilibili', 'login_required'),
      account('c', 'bilibili', 'refresh_failing'),
    ]);

    fireEvent.click(
      await screen.findByRole('button', { name: '2 accounts need attention' }),
    );

    const bilibili = screen.getByRole('region', { name: 'Bilibili' });
    expect(within(bilibili).getAllByRole('link')).toHaveLength(2);
    expect(
      within(bilibili).getByText('Refresh failed 3 times'),
    ).toBeInTheDocument();
    expect(within(bilibili).getByText('Not checked yet')).toBeInTheDocument();
  });
});
