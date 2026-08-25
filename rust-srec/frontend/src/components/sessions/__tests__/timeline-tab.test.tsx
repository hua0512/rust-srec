import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { TimelineTab } from '../timeline-tab';

describe('TimelineTab', () => {
  it('renders a known session-ended event when its optional payload is absent', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    render(
      <I18nProvider i18n={i18n}>
        <TimelineTab
          session={{
            titles: [],
            events: [
              {
                kind: 'session_ended',
                occurred_at: '2026-08-25T12:08:39Z',
                payload: null,
              },
            ],
          }}
        />
      </I18nProvider>,
    );

    expect(screen.getByText('SESSION ENDED')).toBeInTheDocument();
    expect(
      screen.getByText('Session ended. Additional details are unavailable.'),
    ).toBeInTheDocument();
    expect(screen.queryByText('Unrecognised event kind.')).toBeNull();
  });
});
