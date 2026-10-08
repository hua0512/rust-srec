import { useForm } from 'react-hook-form';
import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react';
import {
  CredentialSettings,
  CredentialSettingsField,
} from '../credential-settings-field';
import {
  listCredentialProfiles,
  getEffectiveCredentialSelection,
} from '@/server/functions/credential-profiles';
import type {
  CredentialOwner,
  CredentialPlatform,
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

const menuName = 'More actions for Account A';
const rowName = 'Details for Account A';

function settings(scope: CredentialOwner, platformName = 'Bilibili') {
  mockPlatformCapabilities(platformName);
  return (
    <CredentialSettings
      scope={scope}
      platform={{ id: 'platform-a', name: platformName }}
      selection={{ mode: 'fixed', credential_id: profile.id }}
      onSelectionChange={vi.fn()}
    />
  );
}

it('a streamer selects platform accounts without managing them', async () => {
  renderCredentials(settings(streamerOwner));
  fireEvent.click(await screen.findByRole('button', { name: rowName }));
  expect(screen.getByText('Validation not supported')).toBeInTheDocument();
  for (const name of ['Add account', menuName, 'Log in again', 'Edit'])
    expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
});

it.each([
  ['a template with accounts', templateOwner, true],
  ['a template without accounts', templateOwner, false],
  ['a streamer with accounts', streamerOwner, true],
  ['a streamer without accounts', streamerOwner, false],
])(
  '%s links to the platform settings once, even when the selection is unavailable',
  async (_, scope, hasAccounts) => {
    vi.mocked(listCredentialProfiles).mockResolvedValue(
      hasAccounts ? [accountDetail()] : [],
    );
    vi.mocked(getEffectiveCredentialSelection).mockResolvedValue({
      configured: { mode: 'fixed', credential_id: profile.id },
      resolved: null,
      candidates: [],
      unavailable_reason: 'login_required',
    });
    renderCredentials(settings(scope));
    expect(await screen.findByRole('status')).toHaveTextContent(
      'This account needs a new login.',
    );
    expect(screen.getAllByText('Open platform settings')).toHaveLength(1);
  },
);

it('every scope shows one panel: its selection, then the accounts', async () => {
  for (const scope of [platformOwner, templateOwner, streamerOwner]) {
    renderCredentials(settings(scope));
    const row = await screen.findByRole('button', { name: rowName });
    expect(
      screen.getAllByRole('heading').map((heading) => heading.textContent),
    ).toEqual(['Accounts']);
    const change = await screen.findByRole('button', { name: 'Change' });
    expect(
      change.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.getByRole('heading').closest('section')).toContainElement(
      change,
    );
    cleanup();
  }
});

it('scopes on one platform share a single fetch of its accounts', async () => {
  renderCredentials(
    <>
      {settings(platformOwner)}
      {settings(templateOwner)}
      {settings(streamerOwner)}
    </>,
  );
  await waitFor(() =>
    expect(screen.getAllByRole('button', { name: rowName })).toHaveLength(3),
  );
  expect(listCredentialProfiles).toHaveBeenCalledTimes(1);
  expect(listCredentialProfiles).toHaveBeenCalledWith({
    data: { platform_id: 'platform-a' },
  });
  // Each scope still reads what its own saved configuration selects.
  expect(getEffectiveCredentialSelection).toHaveBeenCalledTimes(3);
  for (const [scope_type, scope_id] of [
    ['platform', 'platform-a'],
    ['template', 'template-a'],
    ['streamer', 'streamer-a'],
  ])
    expect(getEffectiveCredentialSelection).toHaveBeenCalledWith({
      data: { scope_type, scope_id, platform_id: 'platform-a' },
    });
});

it('Streamlink accounts are chosen per streamer, not on the platform or a template', async () => {
  for (const owner of [platformOwner, templateOwner]) {
    renderCredentials(settings(owner, 'streamlink'));
    expect(
      await screen.findByRole('button', { name: rowName }),
    ).toBeInTheDocument();
    expect(screen.queryByText('Account selection')).not.toBeInTheDocument();
    expect(
      screen.getByText(/so its accounts are chosen on each streamer/),
    ).toBeInTheDocument();
    cleanup();
  }
  // The platform page still manages Streamlink accounts.
  renderCredentials(settings(platformOwner, 'streamlink'));
  expect(
    await screen.findByRole('button', { name: 'Add account' }),
  ).toBeInTheDocument();
});

type FieldForm = ReturnType<typeof useForm<{ credential_selection?: unknown }>>;

function Field({
  scope,
  platform,
  defaultSelection,
  onForm,
}: {
  scope?: CredentialOwner;
  platform?: CredentialPlatform;
  defaultSelection?: unknown;
  onForm?: (form: FieldForm) => void;
}) {
  const form = useForm<{ credential_selection?: unknown }>({
    defaultValues: { credential_selection: defaultSelection },
  });
  onForm?.(form);
  return (
    <CredentialSettingsField
      form={form}
      name="credential_selection"
      scope={scope}
      platform={platform}
    />
  );
}

it('marks the selection unsaved until the form is saved with it', async () => {
  mockPlatformCapabilities('Bilibili');
  let form: FieldForm | undefined;
  // The saved pool lists its keys in another order than the form builds them.
  const saved = {
    max_attempts: 3,
    failover: true,
    strategy: 'priority',
    credential_ids: [profile.id],
    mode: 'pool',
  };
  renderCredentials(
    <Field
      scope={{ type: 'streamer', streamer_id: 'streamer-a' }}
      platform={{ id: 'platform-a', name: 'Bilibili' }}
      defaultSelection={saved}
      onForm={(value) => {
        form = value;
      }}
    />,
  );
  await screen.findByRole('button', { name: 'Change' });
  expect(screen.queryByText('Unsaved')).not.toBeInTheDocument();
  act(() =>
    form?.setValue(
      'credential_selection',
      { mode: 'none' },
      {
        shouldDirty: true,
      },
    ),
  );
  expect(await screen.findByText('Unsaved')).toBeInTheDocument();
  act(() => form?.reset({ credential_selection: { mode: 'none' } }));
  await waitFor(() =>
    expect(screen.queryByText('Unsaved')).not.toBeInTheDocument(),
  );
});

it('asks to save a new template or streamer before choosing accounts', () => {
  renderCredentials(
    <Field platform={{ id: 'platform-a', name: 'Bilibili' }} />,
  );
  expect(
    screen.getByText(
      'Save this template or streamer before choosing its accounts.',
    ),
  ).toBeInTheDocument();
  expect(listCredentialProfiles).not.toHaveBeenCalled();
});

it('waits for the caller to resolve the platform', () => {
  renderCredentials(
    <Field scope={{ type: 'template', template_id: 'template-a' }} />,
  );
  expect(
    screen.getByText('Choose a single platform to manage its accounts.'),
  ).toBeInTheDocument();
  expect(listCredentialProfiles).not.toHaveBeenCalled();
});
