import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { TimelineTab } from '../timeline-tab';
import { renderWithProviders } from '@/test/render';

describe('TimelineTab', () => {
  // `from_hysteresis` decides between the "RESUMED START" and "SESSION STARTED"
  // badges, so a missing payload must not resolve to either — that would claim
  // a resumed session was a fresh one, or the reverse.
  it('does not claim a start kind when session_started has no payload', () => {
    renderWithProviders(
      <TimelineTab
        session={{
          titles: [],
          events: [
            {
              kind: 'session_started',
              occurred_at: '2026-08-25T12:08:39Z',
              payload: null,
            },
          ],
        }}
      />,
    );

    expect(screen.getByText('STARTED')).toBeInTheDocument();
    expect(screen.queryByText('SESSION STARTED')).toBeNull();
    expect(screen.queryByText('RESUMED START')).toBeNull();
    expect(
      screen.getByText('Session started. Additional details are unavailable.'),
    ).toBeInTheDocument();
    expect(screen.queryByText('Unrecognised event kind.')).toBeNull();
  });
});
