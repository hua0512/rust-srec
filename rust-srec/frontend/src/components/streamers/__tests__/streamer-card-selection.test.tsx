import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { StreamerCard } from '../streamer-card';

vi.mock('@/store/downloads', () => ({
  useDownloadStore: (selector: (state: unknown) => unknown) =>
    selector({
      getDownloadsByStreamer: () => [],
      getQueuedForStreamer: () => undefined,
    }),
}));

vi.mock('@/components/streamers/card/use-streamer-status', () => ({
  useStreamerStatus: () => ({ label: 'offline' }),
}));

vi.mock('@/components/streamers/card/stream-status-badge', () => ({
  StatusBadge: () => <span>Status</span>,
}));

vi.mock('@/components/streamers/card/stream-avatar-info', () => ({
  StreamAvatarInfo: () => <span>Streamer</span>,
}));

vi.mock('@/components/streamers/card/stream-actions-menu', () => ({
  StreamActionsMenu: () => <button>Actions</button>,
}));

const streamer = {
  id: 'streamer-1',
  name: 'Streamer One',
  url: 'https://example.com/streamer',
  platform_config_id: 'platform-1',
  state: 'NOT_LIVE' as const,
  priority: 'NORMAL' as const,
  enabled: true,
  consecutive_error_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

describe('StreamerCard selection mode', () => {
  const onSelectionChange = vi.fn();

  beforeEach(() => {
    onSelectionChange.mockClear();
  });

  it('toggles selection with pointer and keyboard input', () => {
    const { rerender } = render(
      <StreamerCard
        streamer={streamer}
        selectionMode
        isSelected={false}
        onSelectionChange={onSelectionChange}
        onDelete={vi.fn()}
        onToggle={vi.fn()}
        onCheck={vi.fn()}
      />,
    );

    const card = screen.getByRole('checkbox');
    expect(card).toHaveAttribute('aria-checked', 'false');
    fireEvent.click(card);
    expect(onSelectionChange).toHaveBeenCalledWith('streamer-1', true);
    expect(screen.queryByRole('button', { name: 'Actions' })).toBeNull();

    rerender(
      <StreamerCard
        streamer={streamer}
        selectionMode
        isSelected
        onSelectionChange={onSelectionChange}
        onDelete={vi.fn()}
        onToggle={vi.fn()}
        onCheck={vi.fn()}
      />,
    );

    fireEvent.keyDown(screen.getByRole('checkbox'), { key: 'Enter' });
    expect(onSelectionChange).toHaveBeenLastCalledWith('streamer-1', false);
  });

  it('preserves the normal card actions outside selection mode', () => {
    render(
      <StreamerCard
        streamer={streamer}
        onDelete={vi.fn()}
        onToggle={vi.fn()}
        onCheck={vi.fn()}
      />,
    );

    expect(screen.queryByRole('checkbox')).toBeNull();
    expect(screen.getByRole('button', { name: 'Actions' })).toBeVisible();
  });
});
