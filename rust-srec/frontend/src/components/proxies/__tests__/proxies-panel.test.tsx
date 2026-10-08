import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import {
  createProxy,
  deleteProxy,
  getProxy,
  getSystemProxy,
  listProxies,
  testProxy,
  updateProxy,
} from '@/server/functions/proxies';
import { ProxiesPanel } from '../proxies-panel';
import {
  backendError,
  renderWithProviders,
  savedProxy,
  socksProxy,
} from './proxy-test-utils';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}));
vi.mock('@/server/functions/proxies');

const office = savedProxy();

const noReferences = {
  global: false,
  platforms: [],
  templates: [],
  streamers: [],
  accounts: [],
};

beforeEach(() => {
  vi.clearAllMocks();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.mocked(listProxies).mockResolvedValue([office, socksProxy]);
  vi.mocked(getSystemProxy).mockResolvedValue({
    detected: true,
    url: 'http://127.0.0.1:3128',
    authenticated: false,
    no_proxy: null,
  });
  vi.mocked(getProxy).mockResolvedValue({
    proxy: office,
    references: noReferences,
  });
  vi.mocked(updateProxy).mockResolvedValue(office);
});

async function show() {
  renderWithProviders(<ProxiesPanel />);
  await screen.findByText('Office');
}

function row(name: string) {
  return screen.getByText(name, { selector: 'p' }).closest('li') as HTMLElement;
}

async function openMenuItem(proxy: string, item: string) {
  fireEvent.pointerDown(
    screen.getByRole('button', { name: `More actions for ${proxy}` }),
    { button: 0, ctrlKey: false },
  );
  fireEvent.click(await screen.findByRole('menuitem', { name: item }));
  // Dialogs open once the menu has closed and handed back focus.
  await waitFor(() =>
    expect(screen.queryByRole('menu')).not.toBeInTheDocument(),
  );
  await act(() => new Promise((resolve) => setTimeout(resolve, 0)));
}

it('lists saved proxies with their scheme, address, login and usage', async () => {
  await show();
  expect(row('Office')).toHaveTextContent(
    'http · proxy.example:8080 · alice · not used',
  );
  expect(row('Tunnel')).toHaveTextContent(
    'socks5h · 10.0.0.2:1080 · used by 2',
  );
  expect(await screen.findByText('http://127.0.0.1:3128')).toBeInTheDocument();
});

it('shows an empty state and that no system proxy was detected', async () => {
  vi.mocked(listProxies).mockResolvedValue([]);
  vi.mocked(getSystemProxy).mockResolvedValue({
    detected: false,
    url: null,
    authenticated: false,
    no_proxy: null,
  });
  renderWithProviders(<ProxiesPanel />);
  expect(await screen.findByText('No proxies yet')).toBeInTheDocument();
  expect(
    await screen.findByText('No system proxy detected'),
  ).toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: 'Add proxy' })).toHaveLength(1);
});

it('lists where a proxy is used once its row is expanded', async () => {
  vi.mocked(getProxy).mockResolvedValue({
    proxy: socksProxy,
    references: {
      ...noReferences,
      global: true,
      streamers: [{ id: 's1', name: 'Night owl' }],
    },
  });
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Details for Tunnel' }));
  expect(await screen.findByText('Global settings')).toBeInTheDocument();
  expect(screen.getByText('Night owl')).toBeInTheDocument();
  expect(getProxy).toHaveBeenCalledWith({ data: socksProxy.id });
});

it('an edit keeps the saved password unless a new one is typed', async () => {
  await show();
  await openMenuItem('Office', 'Edit');
  expect(
    screen.getByText('Leave empty to keep the current password.'),
  ).toBeInTheDocument();
  expect(screen.getByLabelText('Password')).toHaveValue('');
  fireEvent.change(screen.getByLabelText('Name'), {
    target: { value: 'Office 2' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  await waitFor(() =>
    expect(updateProxy).toHaveBeenCalledWith({
      data: { id: office.id, expected_version: 3, name: 'Office 2' },
    }),
  );
});

it('an edit can remove the login', async () => {
  await show();
  await openMenuItem('Office', 'Edit');
  fireEvent.click(screen.getByRole('button', { name: 'Remove login' }));
  expect(
    screen.getByText('The saved login will be removed.'),
  ).toBeInTheDocument();
  expect(
    screen.queryByText('Leave empty to keep the current password.'),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  await waitFor(() =>
    expect(updateProxy).toHaveBeenCalledWith({
      data: { id: office.id, expected_version: 3, username: null },
    }),
  );
});

it('a login added to a proxy without one sends its password', async () => {
  await show();
  await openMenuItem('Tunnel', 'Edit');
  fireEvent.change(screen.getByLabelText('Username'), {
    target: { value: 'bob' },
  });
  fireEvent.change(screen.getByLabelText('Password'), {
    target: { value: 's3cret' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  await waitFor(() =>
    expect(updateProxy).toHaveBeenCalledWith({
      data: {
        id: socksProxy.id,
        expected_version: 3,
        username: 'bob',
        password: 's3cret',
      },
    }),
  );
});

it('a new proxy is saved with a normalised address and its login', async () => {
  vi.mocked(createProxy).mockResolvedValue(savedProxy({ id: 'new' }));
  await show();
  fireEvent.click(screen.getByRole('button', { name: 'Add proxy' }));
  fireEvent.change(await screen.findByLabelText('Name'), {
    target: { value: 'Lab' },
  });
  fireEvent.change(screen.getByLabelText('Address'), {
    target: { value: 'user:pw@10.0.0.9:3128' },
  });
  expect(
    screen.getByText(
      'Put the login in the username and password fields instead.',
    ),
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Add proxy' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Address'), {
    target: { value: '10.0.0.9:3128' },
  });
  fireEvent.change(screen.getByLabelText('Username'), {
    target: { value: 'user' },
  });
  fireEvent.change(screen.getByLabelText('Password'), {
    target: { value: 'pw' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add proxy' }));
  await waitFor(() =>
    expect(createProxy).toHaveBeenCalledWith({
      data: {
        name: 'Lab',
        url: 'http://10.0.0.9:3128',
        username: 'user',
        password: 'pw',
      },
    }),
  );
});

it('says which proxy already has the name', async () => {
  vi.mocked(updateProxy).mockRejectedValue(
    backendError(409, {
      code: 'PROXY_NAME_TAKEN',
      message: 'Another proxy already has this name',
      details: { name: 'Tunnel' },
    }),
  );
  await show();
  await openMenuItem('Office', 'Edit');
  fireEvent.change(screen.getByLabelText('Name'), {
    target: { value: 'Tunnel' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  expect(
    await screen.findByText('Another proxy is already named “Tunnel”.'),
  ).toBeInTheDocument();
});

it('tests a saved proxy and shows the result', async () => {
  vi.mocked(testProxy).mockResolvedValue({
    ok: true,
    status: 200,
    latency_ms: 42,
    error: null,
  });
  await show();
  fireEvent.click(within(row('Office')).getByRole('button', { name: 'Test' }));
  const dialog = await screen.findByRole('dialog');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Test' }));
  expect(
    await within(dialog).findByText(/Reachable · HTTP 200 · 42 ms/),
  ).toBeInTheDocument();
  expect(testProxy).toHaveBeenCalledWith({
    data: { proxy_id: office.id, platform: 'bilibili' },
  });
});

it('explains a failed test by its kind', async () => {
  vi.mocked(testProxy).mockResolvedValue({
    ok: false,
    status: 407,
    latency_ms: 12,
    error: 'proxy_authentication_required',
  });
  await show();
  fireEvent.click(within(row('Office')).getByRole('button', { name: 'Test' }));
  const dialog = await screen.findByRole('dialog');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Test' }));
  expect(
    await within(dialog).findByText(/The proxy rejected the login · HTTP 407/),
  ).toBeInTheDocument();
});

it('tests an edited proxy with the stored password', async () => {
  vi.mocked(testProxy).mockResolvedValue({
    ok: false,
    status: null,
    latency_ms: 10_000,
    error: 'timeout',
  });
  await show();
  await openMenuItem('Office', 'Edit');
  fireEvent.change(screen.getByLabelText('Address'), {
    target: { value: 'https://proxy.example:8443' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Test' }));
  expect(
    await screen.findByText(/No answer within 10 seconds/),
  ).toBeInTheDocument();
  expect(testProxy).toHaveBeenCalledWith({
    data: {
      proxy_id: office.id,
      url: 'https://proxy.example:8443',
      platform: 'bilibili',
    },
  });
});

it('lists what still uses a proxy when the backend refuses to delete it', async () => {
  vi.mocked(deleteProxy).mockRejectedValue(
    backendError(409, {
      code: 'PROXY_REFERENCED',
      message: 'The proxy is still in use',
      details: {
        references: {
          ...noReferences,
          templates: [{ id: 't1', name: 'Night shift', being_removed: true }],
          accounts: [
            {
              id: 'a1',
              label: 'Main account',
              platform_id: 'p1',
              platform_name: 'bilibili',
            },
          ],
        },
      },
    }),
  );
  await show();
  await openMenuItem('Office', 'Delete');
  fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
  await waitFor(() =>
    expect(deleteProxy).toHaveBeenCalledWith({
      data: { id: office.id, expected_version: 3 },
    }),
  );
  const dialog = await screen.findByRole('dialog', {
    name: 'This proxy is in use',
  });
  expect(within(dialog).getByText('Night shift')).toBeInTheDocument();
  expect(within(dialog).getByText('(being removed)')).toBeInTheDocument();
  expect(within(dialog).getByText('Main account')).toBeInTheDocument();
});

it('shows where a proxy in use is used instead of offering to delete it', async () => {
  vi.mocked(getProxy).mockResolvedValue({
    proxy: socksProxy,
    references: {
      ...noReferences,
      platforms: [{ id: 'p1', name: 'bilibili' }],
    },
  });
  await show();
  await openMenuItem('Tunnel', 'Delete');
  const dialog = await screen.findByRole('dialog', {
    name: 'This proxy is in use',
  });
  expect(within(dialog).getByText('Bilibili')).toBeInTheDocument();
  await act(async () => {});
  expect(deleteProxy).not.toHaveBeenCalled();
});
