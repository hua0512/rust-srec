import { fireEvent, screen } from '@testing-library/react';
import { AccountSelectionSection } from '../account-selection-section';
import {
  listCredentialProfiles,
  getEffectiveCredentialSelection,
} from '@/server/functions/credential-profiles';
import type {
  CredentialOwner,
  CredentialSelection,
} from '@/api/schemas/credential-profiles';
import {
  accountDetail,
  mockCredentialApi,
  mockPlatformCapabilities,
  platformOwner,
  profile,
  renderCredentials,
  streamerOwner,
  templateOwner,
} from './credential-test-utils';
import { optionNames } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/credential-profiles');

beforeEach(mockCredentialApi);

function renderSection(
  platformName = 'Bilibili',
  scope: CredentialOwner = platformOwner,
  selection: CredentialSelection = { mode: 'fixed', credential_id: profile.id },
) {
  mockPlatformCapabilities(platformName);
  renderCredentials(
    <AccountSelectionSection
      scope={scope}
      platformId="platform-a"
      selection={selection}
      onSelectionChange={vi.fn()}
      dirty={false}
    />,
  );
}

const accountB = accountDetail({
  profile: { ...profile, id: 'account-b', label: 'Account B' },
  health: { validity: 'needs_refresh' },
});

/** A line of the summary, once the platform's accounts have loaded. */
async function sentence(text: string | RegExp) {
  const element = await screen.findByText(
    (_, node) =>
      node?.tagName === 'P' &&
      (typeof text === 'string'
        ? node.textContent?.replace(/\s+/g, ' ').trim() === text
        : text.test(node.textContent ?? '')),
  );
  return element;
}

describe('the selection summary', () => {
  const pool = (
    strategy: 'priority' | 'round_robin',
  ): Extract<CredentialSelection, { mode: 'pool' }> => ({
    mode: 'pool',
    credential_ids: [profile.id, 'account-b'],
    strategy,
    failover: true,
    max_attempts: 3,
  });

  beforeEach(() => {
    vi.mocked(listCredentialProfiles).mockResolvedValue([
      accountDetail({ health: { validity: 'valid' } }),
      accountB,
    ]);
  });

  it.each([
    ['platform', platformOwner, "Uses the platform's setting"],
    ['template', templateOwner, "Uses the template's setting"],
  ] as const)(
    'resolves an unchanged inherit selection from the %s',
    async (_, owner, source) => {
      vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
        configured: null,
        resolved: { owner, selection: pool('priority') },
        candidates: [accountDetail(), accountB],
        unavailable_reason: null,
      });
      renderSection('Bilibili', streamerOwner, { mode: 'inherit' });
      await sentence('Recording uses Account A → then Account B');
      await sentence(`${source} · Failover on · up to 3 tries`);
    },
  );
});

it('does not preview inheritance from a selection that is not saved yet', async () => {
  renderSection('Bilibili', streamerOwner, { mode: 'inherit' });
  expect(
    await screen.findByText('Save this configuration to see what it inherits.'),
  ).toBeInTheDocument();
});

it('a Streamlink streamer picks one account and never a pool', async () => {
  renderSection('streamlink', streamerOwner);
  fireEvent.click(await screen.findByRole('button', { name: 'Change' }));
  expect(await optionNames(screen.getAllByRole('combobox')[0])).toEqual([
    "Inherit (the site's account)",
    'No authentication',
    'Fixed account',
  ]);
});

describe('a Streamlink streamer', () => {
  const youtubeAccount = accountDetail({ sites: ['youtube.com'] });
  const site = (host: string, accounts: string[] = []) => ({
    host,
    site: accounts.length ? host : null,
    accounts,
  });

  beforeEach(() => {
    vi.mocked(listCredentialProfiles).mockResolvedValue([youtubeAccount]);
  });

  it('warns about an account set up for other sites', async () => {
    vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
      configured: { mode: 'fixed', credential_id: profile.id },
      resolved: null,
      candidates: [],
      unavailable_reason: null,
      site: site('kick.com'),
    });
    renderSection('streamlink', streamerOwner);
    expect(await screen.findByRole('status')).toHaveTextContent(
      'Account A is set up for youtube.com, but this streamer is on kick.com. Its cookies will be sent to kick.com.',
    );
  });

  it.each([
    ['is set up for its site', ['youtube.com'], [profile.id]],
    ['names no sites', [], []],
  ])(
    'does not warn about an account that %s',
    async (_, sites: string[], accounts: string[]) => {
      vi.mocked(listCredentialProfiles).mockResolvedValue([
        accountDetail({ sites }),
      ]);
      vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
        configured: { mode: 'fixed', credential_id: profile.id },
        resolved: null,
        candidates: [],
        unavailable_reason: null,
        site: site('youtube.com', accounts),
      });
      renderSection('streamlink', streamerOwner);
      await sentence('Recording uses Account A');
      expect(screen.queryByRole('status')).not.toBeInTheDocument();
    },
  );
});
