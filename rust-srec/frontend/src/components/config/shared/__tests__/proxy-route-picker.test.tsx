import { useState } from 'react';
import { fireEvent, screen, waitFor } from '@testing-library/react';
import {
  createProxy,
  getEffectiveRoute,
  getSystemProxy,
  listProxies,
} from '@/server/functions/proxies';
import type { ProxyRoute } from '@/api/schemas/proxies';
import {
  ProxyRoutePicker,
  type ProxyRouteInherit,
} from '../proxy-route-picker';
import {
  backendError,
  chooseOption,
  optionNames,
  renderWithProviders,
  savedProxy,
  socksProxy,
} from '@/components/proxies/__tests__/proxy-test-utils';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}));
vi.mock('@/server/functions/proxies');

const office = savedProxy();
const onChange = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listProxies).mockResolvedValue([office, socksProxy]);
  vi.mocked(getSystemProxy).mockResolvedValue({
    detected: true,
    url: 'http://127.0.0.1:3128',
    authenticated: false,
    no_proxy: null,
  });
});

/** The picker holding its own value, as a form field would. */
function Harness({
  initial,
  inherit,
  ffmpeg,
}: {
  initial?: ProxyRoute;
  inherit?: ProxyRouteInherit;
  ffmpeg?: boolean;
}) {
  const [value, setValue] = useState<ProxyRoute | undefined>(initial);
  return (
    <ProxyRoutePicker
      id="route"
      value={value}
      inherit={inherit}
      ffmpeg={ffmpeg}
      onChange={(route) => {
        onChange(route);
        setValue(route);
      }}
    />
  );
}

const trigger = () => screen.getByRole('combobox');

it('the global route offers no Inherit', async () => {
  renderWithProviders(<Harness initial={{ kind: 'direct' }} />);
  await waitFor(() => expect(listProxies).toHaveBeenCalled());
  const names = await optionNames(trigger());
  expect(names).toEqual([
    'DirectNo proxy',
    'System proxyhttp://127.0.0.1:3128',
    'Officeproxy.example:8080',
    'Tunnel10.0.0.2:1080',
    'Add proxy…',
  ]);
});

it('labels Inherit with the route the next scope out resolves to', async () => {
  vi.mocked(getEffectiveRoute).mockResolvedValue({
    kind: 'proxy',
    proxy: { id: office.id, name: 'Office' },
    source: 'global',
  });
  renderWithProviders(
    <Harness
      initial={{ kind: 'inherit' }}
      inherit={{ kind: 'scope', query: { scope_type: 'global' } }}
    />,
  );
  await waitFor(() =>
    expect(trigger()).toHaveTextContent(
      'Inherit — Office (from global settings)',
    ),
  );
  expect(getEffectiveRoute).toHaveBeenCalledWith({
    data: { scope_type: 'global' },
  });
  const names = await optionNames(trigger());
  expect(names[0]).toBe('Inherit — Office (from global settings)');
});

it.each([
  [
    { kind: 'per-platform' } as const,
    "Inherit — each streamer's platform setting",
  ],
  [{ kind: 'recording' } as const, "Follow the recording's proxy"],
  [{ kind: 'unknown' } as const, 'Inherit'],
])(
  'labels Inherit for %o without asking the backend',
  async (inherit, label) => {
    renderWithProviders(<Harness inherit={inherit} />);
    expect(trigger()).toHaveTextContent(label);
    expect(getEffectiveRoute).not.toHaveBeenCalled();
  },
);

it('chooses a saved proxy by its id', async () => {
  renderWithProviders(<Harness inherit={{ kind: 'unknown' }} />);
  await chooseOption(trigger(), /^Tunnel/);
  expect(onChange).toHaveBeenCalledWith({ kind: 'proxy', id: 'proxy-socks' });
  expect(trigger()).toHaveTextContent('Tunnel');
});

it('warns that System connects directly when no system proxy was detected', async () => {
  vi.mocked(getSystemProxy).mockResolvedValue({
    detected: false,
    url: null,
    authenticated: false,
    no_proxy: null,
  });
  renderWithProviders(<Harness initial={{ kind: 'system' }} />);
  expect(
    await screen.findByText(/No system proxy was detected/),
  ).toBeInTheDocument();
});

it('warns when FFmpeg records through a proxy that is not http', async () => {
  renderWithProviders(
    <Harness ffmpeg initial={{ kind: 'proxy', id: 'proxy-socks' }} />,
  );
  expect(
    await screen.findByText(/FFmpeg only supports http proxies/),
  ).toBeInTheDocument();
});

it('says nothing about FFmpeg for an http proxy', async () => {
  renderWithProviders(
    <Harness ffmpeg initial={{ kind: 'proxy', id: office.id }} />,
  );
  await waitFor(() => expect(trigger()).toHaveTextContent('Office'));
  expect(screen.queryByText(/FFmpeg only supports/)).not.toBeInTheDocument();
});

it('adds a proxy from the picker and chooses it', async () => {
  vi.mocked(createProxy).mockResolvedValue(
    savedProxy({ id: 'proxy-new', name: 'Lab', host: '10.0.0.9', port: 3128 }),
  );
  renderWithProviders(<Harness initial={{ kind: 'direct' }} />);
  await chooseOption(trigger(), /Add proxy/);
  fireEvent.change(await screen.findByLabelText('Name'), {
    target: { value: 'Lab' },
  });
  fireEvent.change(screen.getByLabelText('Address'), {
    target: { value: '10.0.0.9:3128' },
  });
  expect(screen.getByText('Saved as http://10.0.0.9:3128')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Add proxy' }));
  await waitFor(() =>
    expect(onChange).toHaveBeenCalledWith({ kind: 'proxy', id: 'proxy-new' }),
  );
  expect(createProxy).toHaveBeenCalledWith({
    data: { name: 'Lab', url: 'http://10.0.0.9:3128' },
  });
});

it('offers the existing proxy when the new one duplicates it', async () => {
  vi.mocked(createProxy).mockRejectedValue(
    backendError(409, {
      code: 'PROXY_DUPLICATE',
      message: 'Another proxy already uses this address and username',
      details: { name: 'Office' },
    }),
  );
  renderWithProviders(<Harness initial={{ kind: 'direct' }} />);
  await chooseOption(trigger(), /Add proxy/);
  fireEvent.change(await screen.findByLabelText('Name'), {
    target: { value: 'Copy' },
  });
  fireEvent.change(screen.getByLabelText('Address'), {
    target: { value: 'http://proxy.example:8080' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add proxy' }));
  expect(
    await screen.findByText('“Office” already uses this address and username.'),
  ).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Use “Office” instead' }));
  expect(onChange).toHaveBeenCalledWith({ kind: 'proxy', id: office.id });
});
