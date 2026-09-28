import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import type { ReactNode } from 'react';
import { StreamActionsMenu } from './stream-actions-menu';
import { useDownloadStore } from '@/store/downloads';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: ReactNode }) => <a>{children}</a>,
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
  const queryClient = new QueryClient();
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  const tree = (downloadId: string | undefined) => (
    <QueryClientProvider client={queryClient}>
      <I18nProvider i18n={i18n}>
        <StreamActionsMenu
          downloadId={downloadId}
          streamer={streamer}
          onDelete={() => {}}
          onToggle={() => {}}
        />
      </I18nProvider>
    </QueryClientProvider>
  );
  const view = render(tree(undefined));
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
