import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render } from '@testing-library/react';

import { UploadIndicator } from './upload-indicator';

describe('UploadIndicator', () => {
  it('uses the activated provider locale for plural messages', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

    expect(() =>
      render(
        <I18nProvider i18n={i18n}>
          <UploadIndicator
            uploads={[
              {
                jobId: 'job-1',
                streamerId: 'streamer-1',
                sessionId: 'session-1',
                uploader: 'rclone',
                filesTotal: 2,
                startedAtMs: 1n,
                percent: 50,
                lastEventAtMs: Date.now(),
              },
            ]}
          />
        </I18nProvider>,
      ),
    ).not.toThrow();
  });
});
