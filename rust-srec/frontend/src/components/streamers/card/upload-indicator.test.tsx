import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';

import { UploadIndicator } from './upload-indicator';
import { useUploadStore, type UploadView } from '@/store/uploads';

function renderIndicator(uploads: UploadView[]) {
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  useUploadStore.setState({
    uploadsByJobId: new Map(uploads.map((upload) => [upload.jobId, upload])),
  });

  return render(
    <I18nProvider i18n={i18n}>
      <UploadIndicator streamerId="streamer-1" />
    </I18nProvider>,
  );
}

function createUpload(overrides: Partial<UploadView> = {}): UploadView {
  return {
    jobId: 'job-1',
    streamerId: 'streamer-1',
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
});
