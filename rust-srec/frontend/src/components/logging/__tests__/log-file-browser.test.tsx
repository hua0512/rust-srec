import { setupI18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { PropsWithChildren } from 'react';

import { LogFileBrowser } from '../log-file-browser';

const mocks = vi.hoisted(() => ({
  listLogFiles: vi.fn(),
  getLogsDownloadUrl: vi.fn(),
  toast: { info: vi.fn(), success: vi.fn(), error: vi.fn() },
}));

vi.mock('@/server/functions/logging', () => ({
  listLogFiles: mocks.listLogFiles,
  getLogsDownloadUrl: mocks.getLogsDownloadUrl,
}));

vi.mock('sonner', () => ({ toast: mocks.toast }));

vi.mock('@/utils/env', () => ({ BASE_URL: '/api/' }));

vi.mock('motion/react', () => ({
  motion: {
    div: ({ children }: PropsWithChildren) => <div>{children}</div>,
  },
}));

// A descriptor built here carries the same generated id as the component's, so
// this catalog proves the fallback goes through `i18n`.
const failedToDownloadFile = msg`Failed to download file`;
const i18n = setupI18n({
  locale: 'xx',
  messages: { xx: { [String(failedToDownloadFile.id)]: 'ECHEC' } },
});

function renderBrowser() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <I18nProvider i18n={i18n}>
        <LogFileBrowser />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe('LogFileBrowser downloads', () => {
  let clicked: string[];

  beforeEach(() => {
    vi.clearAllMocks();
    clicked = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked.push(this.href);
    });
    mocks.listLogFiles.mockResolvedValue({
      items: [
        {
          date: '2026-09-01',
          filename: 'rust-srec.2026-09-01.log',
          size_bytes: 2048,
        },
      ],
      total: 1,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('downloads a single day as a token-authenticated archive', async () => {
    mocks.getLogsDownloadUrl.mockResolvedValue({
      token: 'tok',
      expires_at: '2026-09-01T00:10:00Z',
    });
    renderBrowser();

    fireEvent.click(
      await screen.findByTitle('Download rust-srec.2026-09-01.log'),
    );

    await waitFor(() => expect(clicked).toHaveLength(1));
    const url = new URL(clicked[0]);
    expect(url.pathname).toBe('/api/logging/archive');
    expect(url.searchParams.get('token')).toBe('tok');
    expect(url.searchParams.get('from')).toBe('2026-09-01');
    expect(url.searchParams.get('to')).toBe('2026-09-01');
  });

  it('shows the translated fallback when the failure has no message', async () => {
    mocks.getLogsDownloadUrl.mockRejectedValue('network down');
    renderBrowser();

    fireEvent.click(
      await screen.findByTitle('Download rust-srec.2026-09-01.log'),
    );

    await waitFor(() =>
      expect(mocks.toast.error).toHaveBeenCalledWith('ECHEC'),
    );
    expect(clicked).toHaveLength(0);
  });
});
