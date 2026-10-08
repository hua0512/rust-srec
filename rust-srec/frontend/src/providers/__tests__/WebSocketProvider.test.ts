import { QueryClient } from '@tanstack/react-query';

import { UploadTerminalStatus } from '@/api/proto/gen/download_progress_pb.js';
import {
  applyCredentialBlock,
  handleUploadTerminal,
} from '../WebSocketProvider';

function terminal(status: UploadTerminalStatus) {
  return {
    jobId: 'job-1',
    streamerId: 'streamer-1',
    status,
    error: status === UploadTerminalStatus.FAILED ? 'quota exceeded' : '',
    filesSucceeded: 1,
    filesFailed: status === UploadTerminalStatus.FAILED ? 2 : 0,
  };
}

describe('handleUploadTerminal', () => {
  it('removes the live upload and invalidates only its durable records', async () => {
    const queryClient = new QueryClient();
    const targetKey = ['pipeline', 'job', 'job-1', 'uploads'] as const;
    const otherKey = ['pipeline', 'job', 'job-2', 'uploads'] as const;
    queryClient.setQueryData(targetKey, { items: [] });
    queryClient.setQueryData(otherKey, { items: [] });
    const removeUpload = vi.fn();
    const failUpload = vi.fn();

    await handleUploadTerminal(
      queryClient,
      terminal(UploadTerminalStatus.COMPLETED),
      { removeUpload, failUpload },
    );

    expect(removeUpload).toHaveBeenCalledExactlyOnceWith('job-1');
    expect(failUpload).not.toHaveBeenCalled();
    expect(queryClient.getQueryState(targetKey)?.isInvalidated).toBe(true);
    expect(queryClient.getQueryState(otherKey)?.isInvalidated).toBe(false);
  });

  it('keeps a failed upload, with its error, instead of removing it', async () => {
    const removeUpload = vi.fn();
    const failUpload = vi.fn();

    await handleUploadTerminal(
      new QueryClient(),
      terminal(UploadTerminalStatus.FAILED),
      { removeUpload, failUpload },
    );

    expect(removeUpload).not.toHaveBeenCalled();
    expect(failUpload).toHaveBeenCalledExactlyOnceWith({
      jobId: 'job-1',
      streamerId: 'streamer-1',
      error: 'quota exceeded',
      filesSucceeded: 1,
      filesFailed: 2,
    });
  });
});

describe('applyCredentialBlock', () => {
  const streamer = (id: string) => ({
    id,
    name: id,
    url: `https://example.com/${id}`,
    platform_config_id: 'platform-bilibili',
    state: 'NOT_LIVE',
    priority: 'NORMAL',
    enabled: true,
    consecutive_error_count: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    credential_blocked: null,
  });

  it('patches the streamer in every cached list and its own page', () => {
    const queryClient = new QueryClient();
    const listKey = ['streamers', 1, 24];
    const dashboardKey = ['streamers', 'active'];
    const filtersKey = ['streamers', 'blocked', 'filters'];
    const filters = [{ id: 'filter' }];
    queryClient.setQueryData(listKey, {
      items: [streamer('blocked'), streamer('other')],
      total: 2,
    });
    queryClient.setQueryData(dashboardKey, { items: [streamer('other')] });
    queryClient.setQueryData(filtersKey, filters);
    queryClient.setQueryData(['streamer', 'blocked'], streamer('blocked'));
    const dashboardBefore = queryClient.getQueryData(dashboardKey);

    applyCredentialBlock(queryClient, {
      streamerId: 'blocked',
      blocked: true,
      reason: 'login_required',
      platformId: 'platform-bilibili',
      sinceMs: 1_700_000_000_000n,
    });

    const block = {
      reason: 'login_required',
      platform_id: 'platform-bilibili',
      since: new Date(1_700_000_000_000).toISOString(),
    };
    const list = queryClient.getQueryData<{
      items: { id: string; credential_blocked: unknown }[];
      total: number;
    }>(listKey);
    expect(list?.total).toBe(2);
    expect(list?.items.map((item) => item.credential_blocked)).toEqual([
      block,
      null,
    ]);
    expect(
      queryClient.getQueryData<{ credential_blocked: unknown }>([
        'streamer',
        'blocked',
      ])?.credential_blocked,
    ).toEqual(block);
    // Lists without the streamer and non-list entries are left alone.
    expect(queryClient.getQueryData(dashboardKey)).toBe(dashboardBefore);
    expect(queryClient.getQueryData(filtersKey)).toBe(filters);

    applyCredentialBlock(queryClient, {
      streamerId: 'blocked',
      blocked: false,
      reason: '',
      platformId: '',
      sinceMs: 0n,
    });
    expect(
      queryClient.getQueryData<{ items: { credential_blocked: unknown }[] }>(
        listKey,
      )?.items[0].credential_blocked,
    ).toBeNull();
  });
});
