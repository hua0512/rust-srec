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

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
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
  // Dialogs open once the menu has closed and handed back focus.
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

it('deletes an account only after confirmation', async () => {
  await show();
  await openMenuItem('Delete');
  const dialog = await screen.findByRole('alertdialog');
  expect(dialog).toHaveTextContent('Delete "Account A"');
  expect(deleteCredentialProfile).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
  await waitFor(() =>
    expect(deleteCredentialProfile).toHaveBeenCalledWith({
      data: { id: 'account-a', expected_version: 2 },
    }),
  );
});

const streamerReference = (index: number) => ({
  owner: { type: 'streamer' as const, streamer_id: `streamer-${index}` },
  name: `Streamer ${index}`,
  platform_id: 'platform-a',
  platform_name: 'bilibili',
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

it('summarises the first users of an account and details them all', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      references: {
        selections: [1, 2, 3, 4, 5].map(streamerReference),
        recordings: [],
      },
    }),
  ]);
  await show();
  expect(summaryOf()).toHaveTextContent('Streamer 1, Streamer 2 +3');
  expect(detailFor('Selected by')).toHaveTextContent(
    'Streamer 1, Streamer 2, Streamer 3, Streamer 4, Streamer 5',
  );
});

it('expands a row from the row itself, not from its actions', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      health: { validity: 'invalid' },
      capabilities: { validate: true, refresh: false, qr_login: false },
    }),
  ]);
  await show('Twitch');
  const toggle = screen.getByRole('button', { name: rowName });
  const panel = document.getElementById(
    toggle.getAttribute('aria-controls') ?? '',
  );
  expect(toggle).toHaveAttribute('aria-expanded', 'false');
  expect(panel?.closest('[inert]')).not.toBeNull();
  fireEvent.click(toggle);
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
  expect(panel?.closest('[inert]')).toBeNull();
  // The row's own buttons do their job without toggling the details.
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
  fireEvent.click(toggle);
  expect(toggle).toHaveAttribute('aria-expanded', 'false');
});

it('summarises why an account needs attention and when it was checked', async () => {
  const now = Date.now();
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      health: {
        validity: 'needs_refresh',
        refresh_failure_count: 4,
        last_check_at: now - 4 * 60 * 60_000,
      },
      references: { selections: [streamerReference(1)], recordings: [] },
    }),
    accountDetail({
      profile: { ...profile, id: 'account-b', label: 'Account B' },
      health: { validity: 'invalid', reason_code: 'login_required' },
    }),
    accountDetail({
      profile: {
        ...profile,
        id: 'account-c',
        label: 'Account C',
        enabled: false,
      },
      health: { validity: 'valid', last_check_at: now - 5 * 24 * 60 * 60_000 },
    }),
  ]);
  await show();
  expect(summaryOf()).toHaveTextContent(
    'Refresh failed 4 times · checked 4h ago',
  );
  expect(summaryOf('Details for Account B')).toHaveTextContent(
    'The platform asked for a new login',
  );
  expect(summaryOf('Details for Account C')).toHaveTextContent(
    'Not used while disabled · checked 5d ago',
  );
  expect(detailFor('Selected by', 'Details for Account C')).toHaveTextContent(
    'Not selected anywhere',
  );
  expect(
    screen.getByText(/Disabled accounts are not used, checked or refreshed/),
  ).toBeInTheDocument();
});

it('shows when an account was used, checked, refreshed and renews', async () => {
  const now = Date.now();
  const minutes = (count: number) => count * 60_000;
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      profile: { ...profile, last_used_at: now - minutes(5) },
      health: {
        validity: 'needs_refresh',
        last_check_at: now - minutes(120),
        last_refresh_at: null,
        refresh_failure_count: 2,
        last_failure_at: now - minutes(30),
        reason_code: 'refresh_failed',
      },
      next_renewal_at: now + minutes(2 * 24 * 60 + 60),
    }),
    accountDetail({
      profile: { ...profile, id: 'account-b', label: 'Account B' },
      next_renewal_at: now - minutes(1),
    }),
  ]);
  await show();
  // Two failed refreshes are not yet the user's problem.
  expect(summaryOf()).toHaveTextContent(
    'Not selected anywhere · used 5m ago · renews in 2d',
  );
  expect(detailFor('Last used')).toHaveTextContent('5 minutes ago');
  expect(detailFor('Last checked')).toHaveTextContent('about 2 hours ago');
  expect(detailFor('Next renewal')).toHaveTextContent('in 2 days');
  expect(detailFor('Problems')).toHaveTextContent(
    '2 failed refreshes in a row · The last refresh failed',
  );
  expect(summaryOf('Details for Account B')).toHaveTextContent('renewal due');
  expect(detailFor('Next renewal', 'Details for Account B')).toHaveTextContent(
    'Due at its next use',
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

it('shows translated health and hides unsupported actions', async () => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([
    accountDetail({
      health: { validity: 'needs_refresh' },
    }),
  ]);
  await show();
  expect(screen.getByText('Needs refresh')).toBeInTheDocument();
  expect(screen.queryByText('needs_refresh')).not.toBeInTheDocument();
  expect(screen.queryByText(profile.id)).not.toBeInTheDocument();
  fireEvent.pointerDown(screen.getByRole('button', { name: menuName }), {
    button: 0,
    ctrlKey: false,
  });
  await screen.findByRole('menuitem', { name: 'Edit' });
  for (const name of ['Validate', 'Log in with QR code', 'Refresh'])
    expect(screen.queryByRole('menuitem', { name })).not.toBeInTheDocument();
});

it('replacement opens empty and a second account does not change the fixed selection', async () => {
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
  chooseSignIn('Paste cookies');
  expect(screen.getByLabelText('Cookies')).toHaveValue('');
  expect(screen.getByLabelText('Refresh token')).toHaveValue('');
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'Account B' },
  });
  fireEvent.change(screen.getByLabelText('Cookies'), {
    target: { value: 'sid=b' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save account' }));
  await waitFor(() =>
    expect(createCredentialProfile).toHaveBeenCalledWith({
      data: {
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
async function chooseProxy(option: RegExp) {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  const trigger = screen.getByLabelText('Proxy');
  act(() => trigger.focus());
  fireEvent.keyDown(trigger, { key: 'ArrowDown' });
  fireEvent.click(await screen.findByRole('option', { name: option }));
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

it('an account that follows the recording says so in its details', async () => {
  await show();
  expect(
    screen.queryByRole('img', { name: 'Own proxy setting' }),
  ).not.toBeInTheDocument();
  expect(detailFor('Proxy')).toHaveTextContent('Follows the recording');
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

it('the edit dialog does not select the label', async () => {
  await show();
  await openMenuItem('Edit');
  const label = await screen.findByLabelText('Label');
  await waitFor(() => expect(label).toHaveFocus());
  const input = label as HTMLInputElement;
  expect(input.selectionStart).toBe(input.value.length);
  expect(input.selectionEnd).toBe(input.value.length);
});

it.each([
  ['Bilibili', /scanning a QR code/],
  ['Twitch', /with its cookies or tokens/],
])('%s without accounts invites adding one', async (platformName, help) => {
  vi.mocked(listCredentialProfiles).mockResolvedValue([]);
  renderPlatformPage(platformName);
  expect(await screen.findByText('No accounts yet')).toBeInTheDocument();
  expect(screen.getByText(help)).toBeInTheDocument();
  // The empty list offers the only add button.
  expect(screen.getAllByRole('button', { name: 'Add account' })).toHaveLength(
    1,
  );
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

  it('accounts on other platforms name no sites', async () => {
    await show();
    fireEvent.click(screen.getByRole('button', { name: 'Add account' }));
    expect(screen.queryByLabelText('Sites')).not.toBeInTheDocument();
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

  it('the list says which sites an account is for', async () => {
    vi.mocked(listCredentialProfiles).mockResolvedValue([
      accountDetail({ sites: ['kick.com', 'youtube.com'] }),
    ]);
    await show('streamlink');
    expect(summaryOf()).toHaveTextContent('For kick.com, youtube.com');
    expect(summaryOf()).not.toHaveTextContent('Not selected anywhere');
    expect(detailFor('Sites')).toHaveTextContent('kick.com, youtube.com');
  });
});
