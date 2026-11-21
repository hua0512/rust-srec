import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  summarizeUploads,
  UploadStatusIndicator,
} from '../upload-status-indicator';
import { cancelActivePipelineJob } from '@/server/functions/pipeline';
import { useUploadStore, type UploadView } from '@/store/uploads';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/pipeline', () => ({
  cancelActivePipelineJob: vi.fn(),
}));

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
  return renderWithProviders(<UploadStatusIndicator />);
}

describe('UploadStatusIndicator', () => {
  beforeEach(() => {
    useUploadStore.getState().clearAll();
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
