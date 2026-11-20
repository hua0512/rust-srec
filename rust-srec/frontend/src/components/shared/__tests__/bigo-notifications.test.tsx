import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { BigoNotificationStatus } from '@/api/schemas/platform-configs';
import { getBigoNotificationStatus } from '@/server/functions';
import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import { newQueryClient, renderWithProviders } from '@/test/render';
import {
  BigoNotificationsAlert,
  BigoPollingNotice,
} from '../bigo-notifications';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/server/functions')>()),
  getBigoNotificationStatus: vi.fn(),
}));

const BIGO_URL = 'https://www.bigo.tv/100000001';

const REASONS = {
  connected: null,
  connecting: 'disconnected',
  error: 'no_account',
  disabled: 'notifications_off',
} as const;

function listener(
  state: BigoNotificationStatus['state'],
  streamers: BigoNotificationStatus['streamers'] = [],
): BigoNotificationStatus {
  return {
    platform_id: 'platform-bigo',
    state,
    reason: REASONS[state],
    message: null,
    subscribed: 0,
    total: 0,
    streamers,
    missed_pushes: 0,
    last_missed_at: null,
    down_since: null,
  };
}

function renderNotice(status: BigoNotificationStatus, streamerId?: string) {
  const client = newQueryClient();
  client.setQueryData(bigoNotificationStatusQueryOptions.queryKey, status);
  return renderWithProviders(
    <BigoPollingNotice url={BIGO_URL} streamerId={streamerId} />,
    { client },
  );
}

const warning = /checked by polling, which is inconsistent/;

describe('BigoPollingNotice', () => {
  beforeEach(() => vi.mocked(getBigoNotificationStatus).mockReset());

  it('warns about a Bigo URL while notifications are not connected', async () => {
    vi.mocked(getBigoNotificationStatus).mockResolvedValue(listener('error'));
    renderWithProviders(<BigoPollingNotice url={BIGO_URL} />);

    expect(await screen.findByText(warning)).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'Set up live notifications.' }),
    ).toHaveAttribute('href', '/config/platforms/platform-bigo');
  });

  it('stays hidden while notifications are connected or connecting', () => {
    for (const state of ['connected', 'connecting'] as const) {
      const { container, unmount } = renderNotice(listener(state));
      expect(container).toBeEmptyDOMElement();
      unmount();
    }
  });

  it('judges a saved streamer by its own row', () => {
    const status = listener('connected', [
      { streamer_id: 'bigo-1', method: 'polling', reason: 'uid_unknown' },
      { streamer_id: 'bigo-2', method: 'polling', reason: 'subscribing' },
    ]);

    const unknown = renderNotice(status, 'bigo-1');
    expect(unknown.container.textContent).toMatch(warning);
    unknown.unmount();

    // A follow BIGO has not confirmed yet passes on its own.
    expect(renderNotice(status, 'bigo-2').container).toBeEmptyDOMElement();
  });

  it('ignores other platforms without asking for the status', () => {
    const { container } = renderWithProviders(
      <BigoPollingNotice url="https://www.twitch.tv/someone" />,
    );

    expect(container).toBeEmptyDOMElement();
    expect(getBigoNotificationStatus).not.toHaveBeenCalled();
  });
});

describe('BigoNotificationsAlert', () => {
  function renderAlert(status: BigoNotificationStatus) {
    const client = newQueryClient();
    client.setQueryData(bigoNotificationStatusQueryOptions.queryKey, status);
    return renderWithProviders(<BigoNotificationsAlert hasBigoStreamers />, {
      client,
    });
  }

  it('warns while BIGO sends no notifications or the listener stays disconnected', () => {
    // A brief disconnection, which the server does not report as down yet.
    const brief = renderAlert({ ...listener('error'), reason: 'disconnected' });
    expect(brief.container).toBeEmptyDOMElement();
    brief.unmount();

    const notArriving = renderAlert({
      ...listener('connected'),
      reason: 'notifications_not_arriving',
    });
    expect(
      screen.getByText('BIGO is not sending live notifications'),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'Open the Bigo settings' }),
    ).toHaveAttribute('href', '/config/platforms/platform-bigo');
    notArriving.unmount();

    renderAlert({
      ...listener('error'),
      reason: 'disconnected',
      down_since: '2026-01-01T00:00:00Z',
    });
    expect(
      screen.getByText('Bigo live notifications are disconnected'),
    ).toBeInTheDocument();
  });
});
