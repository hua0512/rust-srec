import type { ReactNode } from 'react';
import { renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { CredentialBlock, Streamer } from '@/api/schemas/streamer';
import type { QueuedEntry } from '@/store/downloads';
import { monitoringState } from './recovery-state';
import { useStreamerStatus } from './use-streamer-status';
import { providersWrapper, renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

const loginRequired: CredentialBlock = {
  reason: 'login_required',
  platform_id: 'platform-bilibili',
  since: '2026-10-01T00:00:00Z',
};

function streamer(overrides: Partial<Streamer> = {}): Streamer {
  return {
    id: 'streamer-1',
    name: 'Streamer One',
    url: 'https://live.bilibili.com/1',
    platform_config_id: 'platform-bilibili',
    state: 'NOT_LIVE',
    priority: 'NORMAL',
    enabled: true,
    consecutive_error_count: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    credential_blocked: loginRequired,
    ...overrides,
  };
}

function status(
  value: Streamer,
  {
    hasActiveDownload = false,
    hasRecoverySignal = false,
    queuedEntry,
  }: {
    hasActiveDownload?: boolean;
    hasRecoverySignal?: boolean;
    queuedEntry?: QueuedEntry;
  } = {},
) {
  const { result } = renderHook(
    () =>
      useStreamerStatus(
        value,
        monitoringState(value, hasRecoverySignal, new Date()),
        hasActiveDownload,
        queuedEntry,
      ),
    { wrapper: providersWrapper() },
  );
  return result.current;
}

function text(node: ReactNode) {
  return renderWithProviders(node).container.textContent;
}

const queued: QueuedEntry = {
  streamerId: 'streamer-1',
  sessionId: 'session-1',
  streamerName: 'Streamer One',
  engineType: 'mesio',
  queuedAtMs: 1n,
  isHighPriority: false,
};

describe('useStreamerStatus credential blocks', () => {
  it('replaces offline, queued and download-less live states', () => {
    expect(text(status(streamer()).label)).toBe('Account needs login');
    expect(text(status(streamer({ state: 'LIVE' })).label)).toBe(
      'Account needs login',
    );
    expect(
      text(status(streamer({ state: 'LIVE' }), { queuedEntry: queued }).label),
    ).toBe('Account needs login');
    expect(
      text(
        status(
          streamer({
            credential_blocked: {
              ...loginRequired,
              reason: 'profiles_disabled',
            },
          }),
        ).label,
      ),
    ).toBe('Accounts disabled');
    for (const [reason, label] of [
      ['attempts_exhausted', 'Accounts failing'],
      ['bound_profile_unavailable', 'Account unavailable'],
      ['unknown', 'Account unavailable'],
    ] as const)
      expect(
        text(
          status(streamer({ credential_blocked: { ...loginRequired, reason } }))
            .label,
        ),
      ).toBe(label);
  });

  it('links its tooltip to the platform accounts', () => {
    const { container } = renderWithProviders(status(streamer()).tooltip);
    expect(container.textContent).toContain('Recording is paused');
    expect(container.querySelector('a')?.getAttribute('href')).toBe(
      '/config/platforms/platform-bilibili',
    );
  });

  it('keeps states that matter more', () => {
    // A running recording keeps its account.
    expect(
      text(
        status(streamer({ state: 'LIVE' }), { hasActiveDownload: true }).label,
      ),
    ).toBe('Live');
    expect(
      text(
        status(
          streamer({
            state: 'LIVE',
            consecutive_error_count: 1,
          }),
          { hasActiveDownload: true, hasRecoverySignal: true },
        ).label,
      ),
    ).toBe('Recovering');
    expect(
      text(status(streamer({ state: 'DISABLED', enabled: false })).label),
    ).toBe('Monitoring Stopped');
    expect(
      text(
        status(
          streamer({
            state: 'TEMPORAL_DISABLED',
            disabled_until: new Date(Date.now() + 60_000).toISOString(),
          }),
        ).label,
      ),
    ).toBe('Temporarily Paused');
    expect(text(status(streamer({ state: 'NOT_FOUND' })).label)).toBe(
      'Streamer not found',
    );
  });

  it('is absent without a block', () => {
    expect(text(status(streamer({ credential_blocked: null })).label)).toBe(
      'Offline',
    );
  });
});
