import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { BigoNotificationStatus } from '@/api/schemas/platform-configs';
import { getBigoNotificationStatus } from '@/server/functions';
import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import { newQueryClient, renderWithProviders } from '@/test/render';
import { BigoPollingNotice } from '../bigo-notifications';

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
