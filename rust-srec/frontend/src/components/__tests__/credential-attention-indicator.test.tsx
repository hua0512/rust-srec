import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { CredentialAttentionIndicator } from '../credential-attention-indicator';
import type { CredentialAttention } from '@/api/schemas/credential-profiles';
import { listCredentialAttention } from '@/server/functions/credential-profiles';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/credential-profiles', () => ({
  listCredentialAttention: vi.fn(),
}));

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
  return renderWithProviders(<CredentialAttentionIndicator />);
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
