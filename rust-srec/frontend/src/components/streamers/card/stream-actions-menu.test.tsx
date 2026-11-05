import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import { StreamActionsMenu } from './stream-actions-menu';
import { useDownloadStore } from '@/store/downloads';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));
vi.mock('@/server/functions/downloads', () => ({
  requestLosslessCut: vi.fn(),
}));

const streamer = {
  id: 'streamer-1',
  name: 'Streamer',
  url: 'https://example.com/live',
  enabled: true,
} as Parameters<typeof StreamActionsMenu>[0]['streamer'];

afterEach(cleanup);

it('keeps an open menu open when a recording starts and is replaced', () => {
  useDownloadStore.getState().clearAll();
  const tree = (downloadId: string | undefined) => (
    <StreamActionsMenu
      downloadId={downloadId}
      streamer={streamer}
      onDelete={() => {}}
      onToggle={() => {}}
    />
  );
  const view = renderWithProviders(tree(undefined));
  const trigger = screen.getByRole('button', { name: 'Open menu' });
  act(() => {
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  });
  expect(screen.getByRole('menu')).toBeInTheDocument();
  expect(screen.queryByText('Split file now')).toBeNull();

  act(() =>
    useDownloadStore.getState().upsertManualSplit('download-1', {
      supported: true,
      unavailableReason: '',
      requestId: 0n,
      revision: 1n,
      status: 'idle',
      expiryReason: '',
    }),
  );
  view.rerender(tree('download-1'));
  expect(screen.getByRole('menu')).toBeInTheDocument();
  expect(screen.getByText('Split file now')).toBeInTheDocument();

  view.rerender(tree('download-2'));
  view.rerender(tree(undefined));
  expect(screen.getByRole('menu')).toBeInTheDocument();
  expect(screen.queryByText('Split file now')).toBeNull();
});
