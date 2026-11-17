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

function listener(
  state: BigoNotificationStatus['state'],
): BigoNotificationStatus {
  return {
    platform_id: 'platform-bigo',
    state,
    reason: state === 'connected' ? null : 'disconnected',
    message: null,
    subscribed: 0,
    total: 0,
    streamers: [],
  };
}

const warning = /checked by polling, which is inconsistent/;

describe('BigoPollingNotice', () => {
  beforeEach(() => vi.mocked(getBigoNotificationStatus).mockReset());

  it('warns about a Bigo URL while notifications are not connected', async () => {
    vi.mocked(getBigoNotificationStatus).mockResolvedValue(
      listener('connecting'),
    );
    renderWithProviders(<BigoPollingNotice url={BIGO_URL} />);

    expect(await screen.findByText(warning)).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'Set up live notifications.' }),
    ).toHaveAttribute('href', '/config/platforms/platform-bigo');
  });

  it('stays hidden while notifications are connected', () => {
    for (const state of ['connected', 'connecting'] as const) {
      const client = newQueryClient();
      client.setQueryData(
        bigoNotificationStatusQueryOptions.queryKey,
        listener(state),
      );
      const { container, unmount } = renderWithProviders(
        <BigoPollingNotice url={BIGO_URL} />,
        { client },
      );
      // The same render shows the warning for a listener that is not connected.
      expect(container.textContent).toMatch(
        state === 'connected' ? /^$/ : warning,
      );
      unmount();
    }
  });

  it('ignores other platforms without asking for the status', () => {
    const { container } = renderWithProviders(
      <BigoPollingNotice url="https://www.twitch.tv/someone" />,
    );

    expect(container).toBeEmptyDOMElement();
    expect(getBigoNotificationStatus).not.toHaveBeenCalled();
  });
});
