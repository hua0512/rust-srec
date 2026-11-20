import { renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { BigoNotificationStatus } from '@/api/schemas/platform-configs';
import { getBigoNotificationStatus } from '@/server/functions';
import {
  newQueryClient,
  providersWrapper,
  renderWithProviders,
} from '@/test/render';
import {
  MonitoringMethodBadge,
  useMonitoringMethod,
} from './monitoring-method-badge';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/server/functions')>()),
  getBigoNotificationStatus: vi.fn(),
}));

const BIGO_URL = 'https://www.bigo.tv/100000001';

const listener: BigoNotificationStatus = {
  platform_id: 'platform-bigo',
  state: 'connected',
  reason: null,
  message: null,
  subscribed: 1,
  total: 2,
  streamers: [
    { streamer_id: 'bigo-1', method: 'notifications', reason: null },
    { streamer_id: 'bigo-2', method: 'polling', reason: 'uid_unknown' },
  ],
  missed_pushes: 0,
  last_missed_at: null,
  down_since: null,
};

describe('Bigo monitoring method', () => {
  beforeEach(() => vi.mocked(getBigoNotificationStatus).mockReset());

  it('polls a streamer the status does not list yet, for the listener reason', async () => {
    vi.mocked(getBigoNotificationStatus).mockResolvedValue({
      ...listener,
      state: 'error',
      reason: 'no_account',
    });
    const { result } = renderHook(() => useMonitoringMethod('bigo-new'), {
      wrapper: providersWrapper(newQueryClient()),
    });

    await waitFor(() => expect(result.current).not.toBeNull());
    expect(result.current).toMatchObject({
      method: 'polling',
      reason: 'no_account',
    });
  });

  it('labels the badge by method', async () => {
    vi.mocked(getBigoNotificationStatus).mockResolvedValue(listener);
    const bigo = (id: string) => ({ id, url: BIGO_URL, enabled: true });

    const notifications = renderWithProviders(
      <MonitoringMethodBadge streamer={bigo('bigo-1')} />,
    );
    expect(await notifications.findByText('Notifications')).toBeInTheDocument();

    const polling = renderWithProviders(
      <MonitoringMethodBadge streamer={bigo('bigo-2')} />,
    );
    expect(
      await polling.findByText('Polling · inconsistent'),
    ).toBeInTheDocument();
  });

  it('is absent for other platforms and disabled streamers', () => {
    for (const streamer of [
      { id: 'other', url: 'https://live.bilibili.com/1', enabled: true },
      { id: 'bigo-1', url: BIGO_URL, enabled: false },
    ]) {
      const { container } = renderWithProviders(
        <MonitoringMethodBadge streamer={streamer} />,
      );
      expect(container).toBeEmptyDOMElement();
    }
    // Neither asks for the status.
    expect(getBigoNotificationStatus).not.toHaveBeenCalled();
  });
});
