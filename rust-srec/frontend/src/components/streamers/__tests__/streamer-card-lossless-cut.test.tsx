import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { useDownloadStore } from '@/store/downloads';
import { StreamerCard } from '../streamer-card';
import { renderWithProviders } from '@/test/render';

vi.mock('@/components/streamers/card/stream-avatar-info', () => ({
  StreamAvatarInfo: () => <span>Streamer</span>,
}));
vi.mock('@/components/streamers/card/stream-actions-menu', () => ({
  StreamActionsMenu: () => <button>Actions</button>,
}));
vi.mock('@/components/streamers/progress-indicator', () => ({
  ProgressIndicator: () => <span>Progress</span>,
}));

const streamer = {
  id: 'streamer-1',
  name: 'Streamer One',
  url: 'https://example.com/streamer',
  platform_config_id: 'platform-1',
  state: 'LIVE' as const,
  priority: 'NORMAL' as const,
  enabled: true,
  consecutive_error_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const split = {
  supported: true,
  unavailableReason: '',
  requestId: 1n,
  revision: 2n,
  status: 'idle',
  expiryReason: '',
};

beforeEach(() => {
  useDownloadStore.getState().clearAll();
  useDownloadStore.getState().setSnapshot(
    [
      {
        meta: {
          downloadId: 'dl-1',
          streamerId: 'streamer-1',
          sessionId: 'session-1',
          engineType: 'ffmpeg',
          startedAtMs: 0n,
          updatedAtMs: 0n,
          cdnHost: '',
          downloadUrl: '',
          manualSplit: split,
        },
        metrics: {
          downloadId: 'dl-1',
          status: 'Downloading',
          bytesDownloaded: 0n,
          durationSecs: 0,
          speedBytesPerSec: 0n,
          segmentsCompleted: 0,
          mediaDurationSecs: 0,
          playbackRatio: 0,
        },
      },
    ],
    [],
  );
});
afterEach(cleanup);

it('shows cut progress on the card only while a cut is in progress', () => {
  renderWithProviders(
    <StreamerCard streamer={streamer} onDelete={vi.fn()} onToggle={vi.fn()} />,
  );
  expect(screen.queryByText('Splitting...')).toBeNull();
  const update = (revision: bigint, status: string) =>
    act(() =>
      useDownloadStore
        .getState()
        .upsertManualSplit('dl-1', { ...split, revision, status }),
    );
  update(3n, 'pending');
  expect(screen.getByText('Splitting...')).toBeInTheDocument();
  update(4n, 'finalizing');
  expect(screen.queryByText('Splitting...')).toBeNull();
  expect(screen.getByText('Finalizing...')).toBeInTheDocument();
  update(5n, 'completed');
  expect(screen.queryByText('Finalizing...')).toBeNull();
});
