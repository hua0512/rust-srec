import { fireEvent, screen } from '@testing-library/react';

import { UploadIndicator } from './upload-indicator';
import { useUploadStore, type UploadView } from '@/store/uploads';
import { renderWithProviders } from '@/test/render';

function renderIndicator(uploads: UploadView[]) {
  useUploadStore.setState({
    uploadsByJobId: new Map(uploads.map((upload) => [upload.jobId, upload])),
  });

  return renderWithProviders(<UploadIndicator streamerId="streamer-1" />);
}

function createUpload(overrides: Partial<UploadView> = {}): UploadView {
  return {
    jobId: 'job-1',
    streamerId: 'streamer-1',
    streamerName: 'Streamer One',
    streamerAvatar: '',
    sessionId: 'session-1',
    uploader: 'rclone',
    filesTotal: 2,
    startedAtMs: 1n,
    percent: 50,
    lastEventAtMs: Date.now(),
    ...overrides,
  };
}

describe('UploadIndicator', () => {
  it('shows progress for a single upload', () => {
    renderIndicator([createUpload({ percent: 26.3 })]);

    expect(screen.getByText('26%')).toBeInTheDocument();
  });

  it('shows the upload count when several uploads are active', () => {
    renderIndicator([
      createUpload({ percent: 26.3 }),
      createUpload({ jobId: 'job-2', percent: 100 }),
    ]);

    expect(screen.getByText('2')).toBeInTheDocument();
    expect(screen.queryByText('26%')).not.toBeInTheDocument();
    expect(screen.queryByText('100%')).not.toBeInTheDocument();
  });

  it("shows a retrying upload's attempt in the tooltip", async () => {
    renderIndicator([
      createUpload({
        percent: undefined,
        phase: 'retrying',
        attempt: 2,
        maxAttempts: 3,
      }),
    ]);
    const trigger = document.querySelector('[data-slot="tooltip-trigger"]');
    expect(trigger).not.toBeNull();
    fireEvent.pointerMove(trigger!, { pointerType: 'mouse' });

    const tooltip = await screen.findByRole('tooltip');
    expect(tooltip).toHaveTextContent('Retrying...');
    expect(tooltip).toHaveTextContent('Upload attempt 2 of 3');
  });
});
