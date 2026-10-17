import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { StalledOperationsIndicator } from '../stalled-operations-indicator';
import { StalledOperationBanner } from '../pipeline/jobs/stalled-operation-banner';
import type { StalledOperation } from '@/api/schemas';
import { listStalledOperations } from '@/server/functions/pipeline';

vi.mock('@tanstack/react-router', () => ({
  Link: ({
    children,
    to,
    params,
    className,
  }: {
    children: ReactNode;
    to: string;
    params?: { jobId: string };
    className?: string;
  }) => (
    <a href={to.replace('$jobId', params?.jobId ?? '')} className={className}>
      {children}
    </a>
  ),
}));

vi.mock('@/server/functions/pipeline', () => ({
  listStalledOperations: vi.fn(),
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function operation(
  jobId: string,
  overrides: Partial<StalledOperation> = {},
): StalledOperation {
  return {
    job_id: jobId,
    job_type: 'copy_move',
    processor: 'CopyMoveProcessor',
    worker_type: 'io',
    streamer_id: 'streamer-1',
    input_file_name: 'part.flv',
    input_count: 1,
    ended_at: '2026-01-02T03:04:05.123Z',
    stalled_secs: 125,
    ...overrides,
  };
}

function renderWithProviders(ui: ReactNode, operations: StalledOperation[]) {
  vi.mocked(listStalledOperations).mockResolvedValue(operations);
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nProvider i18n={i18n}>{ui}</I18nProvider>
    </QueryClientProvider>,
  );
}

describe('StalledOperationsIndicator', () => {
  beforeEach(() => {
    vi.mocked(listStalledOperations).mockReset();
  });

  it('renders nothing while every worker is free', async () => {
    const { container } = renderWithProviders(
      <StalledOperationsIndicator />,
      [],
    );
    await waitFor(() => expect(listStalledOperations).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('lists each stalled job with how long it has not responded', async () => {
    renderWithProviders(<StalledOperationsIndicator />, [
      operation('job-a'),
      operation('job-b', { input_file_name: 'first.mp4', input_count: 3 }),
    ]);

    fireEvent.click(
      await screen.findByRole('button', {
        name: '2 file operations are not responding',
      }),
    );

    const links = screen.getAllByRole('link');
    expect(links.map((link) => link.getAttribute('href'))).toEqual([
      '/pipeline/jobs/job-a',
      '/pipeline/jobs/job-b',
    ]);
    expect(
      screen.getAllByText('Not responding for 2m 5s').length,
    ).toBeGreaterThan(0);
    expect(screen.getByText('part.flv')).toBeInTheDocument();
    expect(screen.getByText('first.mp4 and 2 more')).toBeInTheDocument();
  });
});

describe('StalledOperationBanner', () => {
  beforeEach(() => {
    vi.mocked(listStalledOperations).mockReset();
  });

  it('explains an ended job that still holds a worker', async () => {
    renderWithProviders(<StalledOperationBanner jobId="job-a" enabled />, [
      operation('job-a'),
    ]);

    expect(
      await screen.findByText('Still holding a worker'),
    ).toBeInTheDocument();
    expect(screen.getByText('Not responding for 2m 5s')).toBeInTheDocument();
  });

  it('stays hidden for a job that is not stalled', async () => {
    const { container } = renderWithProviders(
      <StalledOperationBanner jobId="job-b" enabled />,
      [operation('job-a')],
    );
    await waitFor(() => expect(listStalledOperations).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('does not query for a job that has not ended', () => {
    const { container } = renderWithProviders(
      <StalledOperationBanner jobId="job-a" enabled={false} />,
      [operation('job-a')],
    );
    expect(listStalledOperations).not.toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });
});
