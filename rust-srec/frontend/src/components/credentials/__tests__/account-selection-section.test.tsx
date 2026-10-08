import { fireEvent, screen, within } from '@testing-library/react';
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

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}));

vi.mock('@/server/functions/credential-profiles');

beforeEach(mockCredentialApi);

function renderSection(
  platformName = 'Bilibili',
  scope: CredentialOwner = platformOwner,
  selection: CredentialSelection = { mode: 'fixed', credential_id: profile.id },
  dirty = false,
) {
  mockPlatformCapabilities(platformName);
  renderCredentials(
    <AccountSelectionSection
      scope={scope}
      platformId="platform-a"
      selection={selection}
      onSelectionChange={vi.fn()}
      dirty={dirty}
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

  it('names the fixed account with its health', async () => {
    renderSection();
    const summary = await sentence('Recording uses Account A');
    expect(
      within(summary).getByRole('img', { name: 'Valid' }),
    ).toBeInTheDocument();
  });

  it('lists primary and backups in order, with failover and attempts', async () => {
    renderSection('Bilibili', platformOwner, pool('priority'));
    const summary = await sentence('Recording uses Account A → then Account B');
    expect(
      within(summary).getByRole('img', { name: 'Needs refresh' }),
    ).toBeInTheDocument();
    await sentence('Failover on · up to 3 tries');
  });

  it('says a round-robin pool rotates between its accounts', async () => {
    renderSection('Bilibili', platformOwner, {
      ...pool('round_robin'),
      failover: false,
      max_attempts: 1,
    });
    await sentence('Recording rotates between Account A, Account B');
    await sentence('Failover off · up to 1 try');
  });

  it('says no account records signed out', async () => {
    renderSection('Bilibili', platformOwner, { mode: 'none' });
    await sentence('No account — records signed out');
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

  it('says an inherit selection that resolves to nothing records signed out', async () => {
    vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
      configured: { mode: 'inherit' },
      resolved: null,
      candidates: [],
      unavailable_reason: null,
    });
    renderSection('Bilibili', templateOwner, { mode: 'inherit' });
    await sentence('No account — records signed out');
    expect(
      screen.getByText('Nothing it inherits from chooses an account'),
    ).toBeInTheDocument();
  });

  it('marks a selection the form has not saved yet', async () => {
    renderSection('Bilibili', platformOwner, { mode: 'none' }, true);
    expect(await screen.findByText('Unsaved')).toBeInTheDocument();
  });

  it('marks nothing while the selection is saved', async () => {
    renderSection();
    await sentence('Recording uses Account A');
    expect(screen.queryByText('Unsaved')).not.toBeInTheDocument();
  });
});

it('expands the editor under the summary with Change', async () => {
  renderSection();
  const change = await screen.findByRole('button', { name: 'Change' });
  const editor = screen.getByText('Account selection');
  expect(change).toHaveAttribute('aria-expanded', 'false');
  expect(editor.closest('[inert]')).not.toBeNull();
  fireEvent.click(change);
  expect(change).toHaveAttribute('aria-expanded', 'true');
  expect(editor.closest('[inert]')).toBeNull();
});

it('does not preview inheritance from a selection that is not saved yet', async () => {
  renderSection('Bilibili', streamerOwner, { mode: 'inherit' });
  expect(
    await screen.findByText('Save this configuration to see what it inherits.'),
  ).toBeInTheDocument();
});

it('points to platform settings when there are no accounts to select', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([]);
  renderSection('Bilibili', templateOwner, { mode: 'inherit' });
  expect(
    await screen.findByText(/This platform has no accounts yet/),
  ).toBeInTheDocument();
});

it('a Streamlink streamer picks one account and never a pool', async () => {
  // Radix Select scrolls the open option list into view; jsdom lacks it.
  HTMLElement.prototype.scrollIntoView = vi.fn();
  renderSection('streamlink', streamerOwner);
  fireEvent.click(await screen.findByRole('button', { name: 'Change' }));
  fireEvent.keyDown(screen.getAllByRole('combobox')[0], { key: 'ArrowDown' });
  expect(
    await screen.findByRole('option', { name: 'Inherit (no account)' }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole('option', { name: 'No authentication' }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole('option', { name: 'Fixed account' }),
  ).toBeInTheDocument();
  for (const name of ['Primary and backups', 'Round-robin pool'])
    expect(screen.queryByRole('option', { name })).not.toBeInTheDocument();
});

it.each<[string, CredentialSelection, string]>([
  [
    'one selected account',
    { mode: 'fixed', credential_id: profile.id },
    'This account needs a new login.',
  ],
  [
    'several selected accounts',
    {
      mode: 'pool',
      credential_ids: [profile.id, 'account-b'],
      strategy: 'priority',
      failover: true,
      max_attempts: 3,
    },
    'All selected accounts need a new login.',
  ],
])(
  'speaks of %s when the selection needs a new login',
  async (_, selection, text) => {
    vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
      configured: selection,
      resolved: { owner: platformOwner, selection },
      candidates: [],
      unavailable_reason: 'login_required',
    });
    renderSection('Bilibili', platformOwner, selection);
    const notice = await screen.findByRole('status');
    expect(notice).toHaveTextContent(text);
    // The notice stays in view while the editor is closed.
    expect(notice.closest('[inert]')).toBeNull();
  },
);

it.each([
  ['attempts_exhausted', 'Signing in with this account kept failing.'],
  ['unknown', 'Signing in is temporarily unavailable.'],
] as const)('explains the %s reason', async (reason, text) => {
  const selection: CredentialSelection = {
    mode: 'fixed',
    credential_id: profile.id,
  };
  vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
    configured: selection,
    resolved: { owner: platformOwner, selection },
    candidates: [],
    unavailable_reason: reason,
  });
  renderSection('Bilibili', platformOwner, selection);
  expect(await screen.findByRole('status')).toHaveTextContent(text);
});
