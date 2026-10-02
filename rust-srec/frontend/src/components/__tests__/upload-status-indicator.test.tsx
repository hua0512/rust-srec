import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  summarizeUploads,
  UploadStatusIndicator,
} from '../upload-status-indicator';
import { cancelActivePipelineJob } from '@/server/functions/pipeline';
import {
  STALE_AFTER_MS,
  useUploadStore,
  type UploadView,
} from '@/store/uploads';

vi.mock('@tanstack/react-router', () => ({
  Link: ({
    children,
    to,
    params,
  }: {
    children: ReactNode;
    to: string;
    params?: { jobId: string };
  }) => <a href={to.replace('$jobId', params?.jobId ?? '')}>{children}</a>,
}));

vi.mock('@/server/functions/pipeline', () => ({
  cancelActivePipelineJob: vi.fn(),
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function createUpload(overrides: Partial<UploadView> = {}): UploadView {
  return {
    jobId: 'job-1',
    streamerId: 'streamer-1',
    streamerName: 'Streamer One',
    streamerAvatar: '',
    sessionId: 'session-1',
    uploader: 'rclone',
    filesTotal: 1,
    startedAtMs: 1n,
    lastEventAtMs: Date.now(),
    ...overrides,
  };
}

function renderIndicator(uploads: UploadView[]) {
  useUploadStore.setState({
    uploadsByJobId: new Map(uploads.map((upload) => [upload.jobId, upload])),
  });
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nProvider i18n={i18n}>
        <UploadStatusIndicator />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe('UploadStatusIndicator', () => {
  beforeEach(() => {
    useUploadStore.getState().clearAll();
  });

  it('renders nothing when no upload is live', () => {
    const { container } = renderIndicator([
      createUpload({ lastEventAtMs: Date.now() - STALE_AFTER_MS }),
    ]);

    expect(container).toBeEmptyDOMElement();
  });

  it("lists every streamer's uploads, each linked to its job", () => {
    renderIndicator([
      createUpload(),
      createUpload({
        jobId: 'job-2',
        streamerId: 'streamer-2',
        streamerName: '',
        startedAtMs: 2n,
      }),
    ]);

    fireEvent.click(screen.getByRole('button', { name: '2 active uploads' }));

    expect(screen.getByText('Streamer One').closest('a')).toHaveAttribute(
      'href',
      '/pipeline/jobs/job-1',
    );
    // No resolved name: the streamer id still identifies the upload.
    expect(screen.getByText('streamer-2').closest('a')).toHaveAttribute(
      'href',
      '/pipeline/jobs/job-2',
    );
  });

  it('keeps a failed upload on screen after the rest finish, until dismissed', async () => {
    renderIndicator([createUpload()]);
    act(() =>
      useUploadStore.getState().fail({
        jobId: 'job-1',
        streamerId: 'streamer-1',
        error: 'quota exceeded',
        filesSucceeded: 0,
        filesFailed: 1,
      }),
    );

    fireEvent.click(screen.getByRole('button', { name: '1 failed upload' }));
    expect(screen.getByText('Streamer One').closest('a')).toHaveAttribute(
      'href',
      '/pipeline/jobs/job-1',
    );
    expect(screen.getByText('quota exceeded')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: /failed upload/ }),
      ).not.toBeInTheDocument(),
    );
  });

  it('cancels an upload only after confirmation', async () => {
    renderIndicator([createUpload()]);

    fireEvent.click(screen.getByRole('button', { name: '1 active upload' }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel upload' }));
    // Opening the dialog closes the popover; the dialog must survive that.
    const dialog = await screen.findByRole('alertdialog');
    expect(cancelActivePipelineJob).not.toHaveBeenCalled();

    fireEvent.click(
      within(dialog).getByRole('button', { name: 'Cancel upload' }),
    );
    await waitFor(() =>
      expect(cancelActivePipelineJob).toHaveBeenCalledExactlyOnceWith({
        data: 'job-1',
      }),
    );
  });

  it('shows queued uploads even before any of them starts', () => {
    renderIndicator([]);
    act(() => useUploadStore.getState().setPendingCount(3));

    fireEvent.click(screen.getByRole('button', { name: '3 queued uploads' }));
    expect(
      screen.getByText('Waiting for a free upload worker.'),
    ).toBeInTheDocument();
  });
});

describe('summarizeUploads', () => {
  it('weights overall progress by bytes, not by upload count', () => {
    const summary = summarizeUploads([
      createUpload({
        percent: 100,
        bytesDone: 1_000n,
        bytesTotal: 1_000n,
        speedBytesPerSec: 10,
        etaSecs: 5,
      }),
      createUpload({
        jobId: 'job-2',
        percent: 0,
        bytesDone: 0n,
        bytesTotal: 9_000n,
        speedBytesPerSec: 30,
        etaSecs: 300,
      }),
    ]);

    expect(summary.percent).toBeCloseTo(10);
    expect(summary.speedBytesPerSec).toBe(40);
    expect(summary.etaSecs).toBe(300);
  });

  it('averages percents when no upload reports sizes', () => {
    expect(
      summarizeUploads([
        createUpload({ percent: 20 }),
        createUpload({ jobId: 'job-2', percent: 60 }),
        createUpload({ jobId: 'job-3' }),
      ]).percent,
    ).toBe(40);
    expect(summarizeUploads([createUpload()]).percent).toBeUndefined();
  });
});
