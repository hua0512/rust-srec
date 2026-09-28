import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import type { ReactNode } from 'react';
import { toast } from 'sonner';
import { useDownloadStore } from '@/store/downloads';
import { requestLosslessCut } from '@/server/functions/downloads';
import { useLosslessCutItem } from './lossless-cut-action';

vi.mock('@/server/functions/downloads', () => ({
  requestLosslessCut: vi.fn(),
}));
vi.mock('sonner', () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenuItem: ({
    children,
    disabled,
    onSelect,
  }: {
    children: ReactNode;
    disabled: boolean;
    onSelect: () => void;
  }) => (
    <button disabled={disabled} onClick={onSelect}>
      {children}
    </button>
  ),
}));

const initial = {
  supported: true,
  unavailableReason: '',
  requestId: 0n,
  revision: 1n,
  status: 'idle',
  expiryReason: '',
};

function Harness({
  downloadId,
  open,
}: {
  downloadId: string | undefined;
  open: boolean;
}) {
  const item = useLosslessCutItem(downloadId);
  return open ? item : <span>Menu closed</span>;
}

function mount() {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  const tree = (open: boolean, downloadId: string | undefined) => (
    <QueryClientProvider client={queryClient}>
      <I18nProvider i18n={i18n}>
        <Harness downloadId={downloadId} open={open} />
      </I18nProvider>
    </QueryClientProvider>
  );
  const view = render(tree(true, 'download-1'));
  return {
    close: () => view.rerender(tree(false, 'download-1')),
    switchTo: (downloadId: string | undefined) =>
      view.rerender(tree(true, downloadId)),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useDownloadStore.getState().clearAll();
  useDownloadStore.getState().setConnectionStatus('connected');
  useDownloadStore.getState().upsertManualSplit('download-1', initial);
});
afterEach(cleanup);

it('is hidden for unsupported recordings and disabled with a visible reason while disconnected', () => {
  useDownloadStore.getState().upsertManualSplit('download-1', {
    ...initial,
    supported: false,
    revision: 2n,
  });
  mount();
  expect(screen.queryByRole('button')).toBeNull();
  act(() =>
    useDownloadStore
      .getState()
      .upsertManualSplit('download-1', { ...initial, revision: 3n }),
  );
  expect(screen.getByRole('button', { name: 'Split file now' })).toBeEnabled();
  expect(screen.queryByText('Reconnect to request a cut.')).toBeNull();
  act(() => useDownloadStore.getState().setConnectionStatus('disconnected'));
  expect(screen.getByRole('button')).toBeDisabled();
  expect(screen.getByText('Reconnect to request a cut.')).toBeVisible();
});

it('is hidden without split state', () => {
  useDownloadStore.getState().clearAll();
  mount();
  expect(screen.queryByRole('button')).toBeNull();
});

it('waits for completed output and reports success after the menu closes', async () => {
  vi.mocked(requestLosslessCut).mockResolvedValue({
    request_id: '1',
    status: 'pending',
  });
  const view = mount();
  fireEvent.click(screen.getByRole('button'));
  await waitFor(() =>
    expect(requestLosslessCut).toHaveBeenCalledWith({ data: 'download-1' }),
  );
  await waitFor(() => expect(screen.getByRole('button')).toBeDisabled());
  expect(toast.success).not.toHaveBeenCalled();
  view.close();
  act(() =>
    useDownloadStore.getState().upsertManualSplit('download-1', {
      ...initial,
      requestId: 1n,
      revision: 3n,
      status: 'finalizing',
    }),
  );
  expect(toast.success).not.toHaveBeenCalled();
  act(() =>
    useDownloadStore.getState().upsertManualSplit('download-1', {
      ...initial,
      requestId: 1n,
      revision: 4n,
      status: 'completed',
    }),
  );
  await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(1));
});

it('restores a pending cut and explains expiration without reporting success', async () => {
  vi.mocked(requestLosslessCut).mockResolvedValue({
    request_id: '1',
    status: 'pending',
  });
  mount();
  fireEvent.click(screen.getByRole('button'));
  await waitFor(() => expect(requestLosslessCut).toHaveBeenCalledTimes(1));
  act(() =>
    useDownloadStore.getState().upsertManualSplit('download-1', {
      ...initial,
      requestId: 1n,
      revision: 3n,
      status: 'pending',
    }),
  );
  expect(
    screen.getByRole('button', { name: /^Waiting for a safe cut\.\.\./ }),
  ).toBeDisabled();
  expect(
    screen.getByText('The file is split at the next safe boundary.'),
  ).toBeVisible();
  act(() =>
    useDownloadStore.getState().upsertManualSplit('download-1', {
      ...initial,
      requestId: 1n,
      revision: 4n,
      status: 'expired',
    }),
  );
  await waitFor(() =>
    expect(toast.error).toHaveBeenCalledWith(
      'No safe cut boundary was found. Recording continues.',
    ),
  );
  expect(screen.getByRole('button', { name: 'Split file now' })).toBeEnabled();
  expect(toast.success).not.toHaveBeenCalled();
});

it('explains why a cut expired', async () => {
  vi.mocked(requestLosslessCut).mockResolvedValue({
    request_id: '1',
    status: 'pending',
  });
  mount();
  fireEvent.click(screen.getByRole('button'));
  await waitFor(() => expect(requestLosslessCut).toHaveBeenCalledTimes(1));
  act(() =>
    useDownloadStore.getState().upsertManualSplit('download-1', {
      ...initial,
      requestId: 1n,
      revision: 4n,
      status: 'expired',
      expiryReason: 'no_independent_segment',
    }),
  );
  await waitFor(() =>
    expect(toast.error).toHaveBeenCalledWith(
      "The stream's segments are not marked as independently playable, so no safe cut boundary was found. Recording continues.",
    ),
  );
});

it('shows the backend error when a request is rejected', async () => {
  vi.mocked(requestLosslessCut).mockRejectedValue(
    new Error('Lossless cutting is unavailable for this recording'),
  );
  mount();
  fireEvent.click(screen.getByRole('button'));
  await waitFor(() =>
    expect(toast.error).toHaveBeenCalledWith(
      'Lossless cutting is unavailable for this recording',
    ),
  );
});

it('falls back to generic text for an error without a message', async () => {
  vi.mocked(requestLosslessCut).mockRejectedValue(new Error(''));
  mount();
  fireEvent.click(screen.getByRole('button'));
  await waitFor(() =>
    expect(toast.error).toHaveBeenCalledWith(
      'Could not request lossless cutting. Refresh the recording status and try again.',
    ),
  );
});

it('does not carry a request over to a replacement recording', async () => {
  vi.mocked(requestLosslessCut).mockResolvedValue({
    request_id: '1',
    status: 'pending',
  });
  const view = mount();
  fireEvent.click(screen.getByRole('button'));
  await waitFor(() => expect(screen.getByRole('button')).toBeDisabled());
  act(() =>
    useDownloadStore
      .getState()
      .upsertManualSplit('download-2', { ...initial, revision: 1n }),
  );
  view.switchTo('download-2');
  expect(screen.getByRole('button', { name: 'Split file now' })).toBeEnabled();
  view.switchTo(undefined);
  expect(screen.queryByRole('button')).toBeNull();
});
