import type { ReactNode } from 'react';
import { fireEvent, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { StalledOperationsIndicator } from '../stalled-operations-indicator';
import { StalledOperationBanner } from '../pipeline/jobs/stalled-operation-banner';
import type { StalledOperation } from '@/api/schemas';
import { listStalledOperations } from '@/server/functions/pipeline';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/pipeline', () => ({
  listStalledOperations: vi.fn(),
}));

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

function renderWith(ui: ReactNode, operations: StalledOperation[]) {
  vi.mocked(listStalledOperations).mockResolvedValue(operations);
  return renderWithProviders(ui);
}

describe('StalledOperationsIndicator', () => {
  beforeEach(() => {
    vi.mocked(listStalledOperations).mockReset();
  });

  it('renders nothing while every worker is free', async () => {
    const { container } = renderWith(<StalledOperationsIndicator />, []);
    await waitFor(() => expect(listStalledOperations).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('lists each stalled job with how long it has not responded', async () => {
    renderWith(<StalledOperationsIndicator />, [
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
    renderWith(<StalledOperationBanner jobId="job-a" enabled />, [
      operation('job-a'),
    ]);

    expect(
      await screen.findByText('Still holding a worker'),
    ).toBeInTheDocument();
    expect(screen.getByText('Not responding for 2m 5s')).toBeInTheDocument();
  });

  it('stays hidden for a job that is not stalled', async () => {
    const { container } = renderWith(
      <StalledOperationBanner jobId="job-b" enabled />,
      [operation('job-a')],
    );
    await waitFor(() => expect(listStalledOperations).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('does not query for a job that has not ended', () => {
    const { container } = renderWith(
      <StalledOperationBanner jobId="job-a" enabled={false} />,
      [operation('job-a')],
    );
    expect(listStalledOperations).not.toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });
});
