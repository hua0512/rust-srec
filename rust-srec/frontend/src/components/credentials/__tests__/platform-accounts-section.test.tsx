import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { CredentialSettings } from '../credential-settings-field';
import {
  createCredentialProfile,
  deleteCredentialProfile,
  generateCredentialLogin,
  updateCredentialProfile,
  listCredentialProfiles,
  getEffectiveCredentialSelection,
  validateCredentialProfile,
} from '@/server/functions/credential-profiles';
import {
  accountDetail,
  mockCredentialApi,
  mockPlatformCapabilities,
  platformOwner,
  profile,
  renderCredentials,
} from './credential-test-utils';
import { getSystemProxy, listProxies } from '@/server/functions/proxies';
import { chooseOption } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/credential-profiles');
vi.mock('@/server/functions/proxies');

const onSelectionChange = vi.fn();
beforeEach(() => {
  mockCredentialApi();
  vi.mocked(listProxies).mockResolvedValue([officeProxy]);
  vi.mocked(getSystemProxy).mockResolvedValue({
    detected: false,
    url: null,
    authenticated: false,
    no_proxy: null,
  });
  vi.mocked(updateCredentialProfile).mockResolvedValue(profile);
  vi.mocked(createCredentialProfile).mockResolvedValue({
    ...profile,
    id: 'account-b',
  });
});

/** The platform page: its accounts above the platform's own selection. */
function renderPlatformPage(platformName = 'Bilibili') {
  mockPlatformCapabilities(platformName);
  return renderCredentials(
    <CredentialSettings
      scope={platformOwner}
      platform={{ id: 'platform-a', name: platformName }}
      selection={{ mode: 'fixed', credential_id: profile.id }}
      onSelectionChange={onSelectionChange}
    />,
  );
}

const menuName = 'More actions for Account A';
const rowName = 'Details for Account A';

/** Expands an account's row and returns its details' value for `term`. */
function detailFor(term: string, row = rowName) {
  const toggle = screen.getByRole('button', { name: row });
  if (toggle.getAttribute('aria-expanded') !== 'true') fireEvent.click(toggle);
  const panel = document.getElementById(
    toggle.getAttribute('aria-controls') ?? '',
  );
  if (!panel) throw new Error(`no details for ${row}`);
  const dt = within(panel).getByText(term, { selector: 'dt' });
  return dt.nextElementSibling as HTMLElement;
}

/** The summary line under an account's name. */
function summaryOf(row = rowName) {
  const item = screen.getByRole('button', { name: row }).closest('li');
  return item?.querySelector('p[title]') as HTMLElement;
}

async function show(platformName = 'Bilibili') {
  renderPlatformPage(platformName);
  await screen.findByRole('button', { name: menuName });
}

/** Picks how a new account signs in; Radix tabs switch on a primary mousedown. */
function chooseSignIn(name: 'Scan QR code' | 'Paste cookies') {
  fireEvent.mouseDown(screen.getByRole('tab', { name }), {
    button: 0,
    ctrlKey: false,
  });
}

async function openMenuItem(name: string) {
  // Radix opens its menu on a primary-button pointerdown.
  fireEvent.pointerDown(screen.getByRole('button', { name: menuName }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(await screen.findByRole('menuitem', { name }));
  // Dialogs open once the menu has closed and handed back focus, which Radix
  // does on a zero-delay timer after the menu unmounts.
  await waitFor(() =>
    expect(screen.queryByRole('menu')).not.toBeInTheDocument(),
  );
  await act(() => new Promise((resolve) => setTimeout(resolve, 0)));
}

it('supports an empty-cookie Twitch account using only its own access token', async () => {
  await show('Twitch');
  fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
  const accessToken = await screen.findByLabelText('Access token');
  expect(screen.queryByLabelText('Refresh token')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Twitch account' },
  });
  fireEvent.change(accessToken, {
    target: { value: 'twitch-token' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(createCredentialProfile).toHaveBeenCalledWith({
      data: {
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
  await openMenuItem('Edit');
  expect(screen.queryByLabelText('Cookies')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Renamed' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(updateCredentialProfile).toHaveBeenCalledWith({
      data: {
        id: 'account-a',
        expected_version: 2,
        label: 'Renamed',
      },
    }),
  );
  expect(onSelectionChange).not.toHaveBeenCalled();
});

it('the edit dialog replaces credentials only when asked', async () => {
  await show();
  await openMenuItem('Edit');
  fireEvent.click(screen.getByRole('switch', { name: /Replace credentials/ }));
  expect(screen.getByLabelText('Cookies')).toHaveValue('');
  fireEvent.change(screen.getByLabelText('Cookies'), {
    target: { value: 'sid=new' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(updateCredentialProfile).toHaveBeenCalledWith({
      data: {
        id: 'account-a',
        expected_version: 2,
        label: 'Account A',
        replacement: {
          cookies: 'sid=new',
          refresh_token: null,
          access_token: null,
          reauth_config: null,
        },
      },
    }),
  );
});

const inUse = {
  selections: [
    {
      owner: { type: 'platform' as const, platform_id: 'platform-a' },
      name: 'bilibili',
      platform_id: 'platform-a',
      platform_name: 'bilibili',
    },
    {
      owner: { type: 'template' as const, template_id: 'template-a' },
      name: 'Night shift',
      platform_id: 'platform-a',
      platform_name: 'bilibili',
    },
  ],
  recordings: [
    {
      session_id: 'session-a',
      streamer_id: 'streamer-a',
      streamer_name: 'Alice',
    },
  ],
};

it('explains why an account in use cannot be deleted', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({ references: inUse }),
  ]);
  await show();
  expect(summaryOf()).toHaveTextContent(
    'Bilibili, Night shift · 1 live recording',
  );
  expect(detailFor('Selected by')).toHaveTextContent('Bilibili, Night shift');
  expect(detailFor('Recording')).toHaveTextContent('Alice');
  await openMenuItem('Delete');
  const dialog = await screen.findByRole('dialog');
  expect(dialog).toHaveTextContent('This account is in use');
  expect(dialog).toHaveTextContent(
    'Remove it from the account selection of 2 configurations.',
  );
  expect(dialog).toHaveTextContent(
    'Wait for the active recording using it to end.',
  );
  expect(dialog).toHaveTextContent('Night shift');
  expect(dialog).toHaveTextContent('Alice');
  expect(
    screen.queryByRole('button', { name: 'Delete' }),
  ).not.toBeInTheDocument();
  // Like the app's other dialogs, it closes from its corner too.
  fireEvent.click(within(dialog).getAllByRole('button', { name: 'Close' })[0]);
  await waitFor(() =>
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
  );
});

it('a refused deletion shows what the backend says still uses the account', async () => {
  vi.mocked(deleteCredentialProfile).mockRejectedValue(
    Object.assign(new Error('Credential profile is still referenced'), {
      status: 409,
      body: {
        code: 'CREDENTIAL_PROFILE_REFERENCED',
        message: 'Credential profile is still referenced',
        details: { references: inUse },
      },
    }),
  );
  await show();
  await openMenuItem('Delete');
  fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
  const dialog = await screen.findByRole('dialog');
  await waitFor(() =>
    expect(dialog).toHaveTextContent('This account is in use'),
  );
  expect(dialog).toHaveTextContent('Night shift');
});

it('disabled accounts cannot start validation or refresh, but can log in again', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      profile: { ...profile, enabled: false },
      health: { validity: 'invalid', reason_code: 'login_required' },
      capabilities: { validate: true, refresh: true, qr_login: true },
    }),
  ]);
  await show();
  // A disabled account needs nothing from the user, as in the header's list.
  expect(
    screen.queryByRole('button', { name: 'Log in again' }),
  ).not.toBeInTheDocument();
  fireEvent.pointerDown(screen.getByRole('button', { name: menuName }), {
    button: 0,
    ctrlKey: false,
  });
  for (const name of ['Validate', 'Refresh'])
    expect(await screen.findByRole('menuitem', { name })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
  expect(
    screen.getByRole('menuitem', { name: 'Log in with QR code' }),
  ).not.toHaveAttribute('aria-disabled');
});

it('an account change refreshes the platform accounts and its effective selections', async () => {
  await show();
  await waitFor(() =>
    expect(getEffectiveCredentialSelection).toHaveBeenCalledTimes(1),
  );
  expect(listCredentialProfiles).toHaveBeenCalledTimes(1);
  await openMenuItem('Disable');
  await waitFor(() =>
    expect(updateCredentialProfile).toHaveBeenCalledWith({
      data: { id: 'account-a', expected_version: 2, enabled: false },
    }),
  );
  await waitFor(() => {
    expect(listCredentialProfiles).toHaveBeenCalledTimes(2);
    expect(getEffectiveCredentialSelection).toHaveBeenCalledTimes(2);
  });
  expect(onSelectionChange).not.toHaveBeenCalled();
});

const officeProxy = {
  id: 'proxy-office',
  name: 'Office',
  url: 'socks5h://proxy.example:1080',
  scheme: 'socks5h',
  host: 'proxy.example',
  port: 1080,
  username: 'user',
  has_password: true,
  version: 1,
  created_at: 0,
  updated_at: 0,
  usage_count: 1,
};

/** Opens the proxy select and picks `option`. */
function chooseProxy(option: RegExp) {
  return chooseOption(screen.getByLabelText('Proxy'), option);
}

/** Opens a new account with a label, routed through the saved proxy. */
async function addProxiedAccount(signIn: 'Scan QR code' | 'Paste cookies') {
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
  chooseSignIn(signIn);
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Proxied' },
  });
  expect(screen.getByLabelText('Proxy')).toHaveTextContent(
    "Follow the recording's proxy",
  );
  await chooseProxy(/^Office/);
}

it('a new account can go through a saved proxy', async () => {
  await addProxiedAccount('Paste cookies');
  fireEvent.change(screen.getByLabelText('Cookies'), {
    target: { value: 'sid=p' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(createCredentialProfile).toHaveBeenCalledWith({
      data: expect.objectContaining({
        label: 'Proxied',
        proxy_route: { kind: 'proxy', id: 'proxy-office' },
      }),
    }),
  );
});

it('a new account that follows the recording sends no route', async () => {
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
  chooseSignIn('Paste cookies');
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Plain' },
  });
  fireEvent.change(screen.getByLabelText('Cookies'), {
    target: { value: 'sid=p' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() => expect(createCredentialProfile).toHaveBeenCalled());
  expect(
    vi.mocked(createCredentialProfile).mock.calls[0][0].data,
  ).not.toHaveProperty('proxy_route');
});

it('a QR sign-in for a new account goes through its proxy', async () => {
  vi.mocked(generateCredentialLogin).mockReturnValue(new Promise(() => {}));
  await addProxiedAccount('Scan QR code');
  // Signing in by QR code needs no pasted credentials.
  expect(screen.queryByLabelText('Cookies')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Show QR code' }));
  await waitFor(() =>
    expect(generateCredentialLogin).toHaveBeenCalledWith({
      data: {
        type: 'create',
        platform_id: 'platform-a',
        label: 'Proxied',
        proxy_route: { kind: 'proxy', id: 'proxy-office' },
      },
    }),
  );
});

it('an edit keeps the proxy unless changed, and can switch to direct', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      profile: {
        ...profile,
        proxy_route: { kind: 'proxy', id: 'proxy-office' },
      },
    }),
  ]);
  await show();
  expect(
    screen.getByRole('img', { name: 'Own proxy setting' }),
  ).toBeInTheDocument();
  await waitFor(() => expect(detailFor('Proxy')).toHaveTextContent('Office'));
  await openMenuItem('Edit');
  expect(screen.getByLabelText('Proxy')).toHaveTextContent('Office');
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(updateCredentialProfile).toHaveBeenCalledWith({
      data: { id: 'account-a', expected_version: 2, label: 'Account A' },
    }),
  );
  vi.mocked(updateCredentialProfile).mockClear();
  await openMenuItem('Edit');
  await chooseProxy(/^Direct/);
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(updateCredentialProfile).toHaveBeenCalledWith({
      data: {
        id: 'account-a',
        expected_version: 2,
        label: 'Account A',
        proxy_route: { kind: 'direct' },
      },
    }),
  );
});

const accountB = { ...profile, id: 'account-b', label: 'Account B' };
const qrCapable = { validate: true, refresh: true, qr_login: true };

it.each([
  ['an invalid login', { validity: 'invalid' as const }],
  [
    'a refresh that keeps failing',
    { validity: 'needs_refresh' as const, refresh_failure_count: 3 },
  ],
])('offers a new QR login for %s', async (_, health) => {
  vi.mocked(generateCredentialLogin).mockReturnValue(new Promise(() => {}));
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({ health, capabilities: qrCapable }),
  ]);
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Log in again' }));
  expect(
    await screen.findByRole('heading', {
      name: 'Log in again to “Account A”',
    }),
  ).toBeInTheDocument();
  await waitFor(() =>
    expect(generateCredentialLogin).toHaveBeenCalledWith({
      data: { type: 'replace', profile_id: 'account-a', expected_version: 2 },
    }),
  );
});

it('keeps the row quiet while a refresh may still fix the account', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      health: { validity: 'needs_refresh', refresh_failure_count: 2 },
      capabilities: qrCapable,
    }),
    accountDetail({
      profile: accountB,
      health: { validity: 'valid' },
      capabilities: qrCapable,
    }),
  ]);
  await show();
  for (const name of ['Log in again', 'Edit', 'Validate', 'QR Login'])
    expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
});

it('asks for new credentials where the platform has no QR login', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      health: { validity: 'invalid' },
      capabilities: { validate: true, refresh: false, qr_login: false },
    }),
  ]);
  await show('Twitch');
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  expect(
    await screen.findByRole('switch', { name: /Replace credentials/ }),
  ).toBeChecked();
  expect(screen.getByLabelText('Access token')).toHaveValue('');
});

it('shows progress only on the row whose action is running', async () => {
  vi.mocked(validateCredentialProfile).mockReturnValue(new Promise(() => {}));
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({ capabilities: qrCapable }),
    accountDetail({ profile: accountB, capabilities: qrCapable }),
  ]);
  await show();
  await openMenuItem('Validate');
  const [rowA, rowB] = screen.getAllByRole('listitem');
  expect(within(rowA).getByRole('status')).toHaveTextContent('Checking…');
  expect(within(rowB).queryByRole('status')).not.toBeInTheDocument();
  fireEvent.pointerDown(
    within(rowB).getByRole('button', { name: 'More actions for Account B' }),
    { button: 0, ctrlKey: false },
  );
  expect(
    await screen.findByRole('menuitem', { name: 'Validate' }),
  ).not.toHaveAttribute('aria-disabled');
  fireEvent.keyDown(document.activeElement ?? document.body, {
    key: 'Escape',
  });
  fireEvent.pointerDown(screen.getByRole('button', { name: menuName }), {
    button: 0,
    ctrlKey: false,
  });
  expect(
    await screen.findByRole('menuitem', { name: 'Validate' }),
  ).toHaveAttribute('aria-disabled', 'true');
});

describe('Streamlink account sites', () => {
  it('a new Streamlink account names the sites it is for', async () => {
    await show('streamlink');
    fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
    fireEvent.change(screen.getByLabelText('Label'), {
      target: { value: 'YouTube' },
    });
    fireEvent.change(screen.getByLabelText('Cookies'), {
      target: { value: 'sid=y' },
    });
    fireEvent.change(screen.getByLabelText('Sites'), {
      target: { value: 'www.youtube.com, kick.com\n m.example.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
    await waitFor(() =>
      expect(createCredentialProfile).toHaveBeenCalledWith({
        data: expect.objectContaining({
          label: 'YouTube',
          sites: ['www.youtube.com', 'kick.com', 'm.example.com'],
        }),
      }),
    );
  });

  it('an edit sends the sites only when they change', async () => {
    vi.mocked(listCredentialProfiles).mockResolvedValue([
      accountDetail({ sites: ['youtube.com'] }),
    ]);
    await show('streamlink');
    await openMenuItem('Edit');
    expect(screen.getByLabelText('Sites')).toHaveValue('youtube.com');
    fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
    await waitFor(() =>
      expect(updateCredentialProfile).toHaveBeenCalledWith({
        data: { id: 'account-a', expected_version: 2, label: 'Account A' },
      }),
    );
    vi.mocked(updateCredentialProfile).mockClear();
    await openMenuItem('Edit');
    fireEvent.change(screen.getByLabelText('Sites'), {
      target: { value: '' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
    await waitFor(() =>
      expect(updateCredentialProfile).toHaveBeenCalledWith({
        data: {
          id: 'account-a',
          expected_version: 2,
          label: 'Account A',
          sites: [],
        },
      }),
    );
  });

  it('names the account that already has a site', async () => {
    vi.mocked(createCredentialProfile).mockRejectedValue(
      Object.assign(new Error('Another account already uses this site'), {
        status: 409,
        body: {
          code: 'CREDENTIAL_SITE_TAKEN',
          message: 'Another account already uses this site',
          details: { site: 'youtube.com', profile_id: 'x', label: 'Main' },
        },
      }),
    );
    await show('streamlink');
    fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
    fireEvent.change(screen.getByLabelText('Label'), {
      target: { value: 'Second' },
    });
    fireEvent.change(screen.getByLabelText('Cookies'), {
      target: { value: 'sid=s' },
    });
    fireEvent.change(screen.getByLabelText('Sites'), {
      target: { value: 'youtube.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
    expect(
      await screen.findByText(
        'youtube.com already belongs to the account Main. A site can belong to one account only.',
      ),
    ).toBeInTheDocument();
  });
});
