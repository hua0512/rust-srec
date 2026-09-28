import {
  TERMINATED_TTL_MS,
  sweepTerminatedDownloads,
  useDownloadStore,
} from '../downloads';

function metricsFor(downloadId: string) {
  return {
    downloadId,
    status: 'Downloading',
    bytesDownloaded: 0n,
    durationSecs: 0,
    speedBytesPerSec: 0n,
    segmentsCompleted: 0,
    mediaDurationSecs: 0,
    playbackRatio: 0,
  };
}

describe('useDownloadStore terminated tracking', () => {
  beforeEach(() => {
    useDownloadStore.getState().clearAll();
  });

  it('restores pending cuts from snapshots and rejects stale or terminal updates', () => {
    const store = useDownloadStore.getState();
    const split = {
      supported: true,
      unavailableReason: '',
      requestId: 1n,
      revision: 3n,
      status: 'pending',
      expiryReason: '',
    };
    store.setSnapshot(
      [
        {
          meta: {
            downloadId: 'dl-1',
            streamerId: 'streamer-1',
            sessionId: 'session-1',
            engineType: 'mesio',
            startedAtMs: 0n,
            updatedAtMs: 0n,
            cdnHost: '',
            downloadUrl: '',
            manualSplit: split,
          },
          metrics: metricsFor('dl-1'),
        },
      ],
      [],
    );
    expect(useDownloadStore.getState().manualSplitById.get('dl-1')).toEqual(
      split,
    );
    store.upsertManualSplit('dl-1', {
      ...split,
      revision: 5n,
      status: 'completed',
    });
    store.upsertManualSplit('dl-1', {
      ...split,
      revision: 4n,
      status: 'finalizing',
    });
    expect(
      useDownloadStore.getState().manualSplitById.get('dl-1')?.status,
    ).toBe('completed');
    store.removeDownload('dl-1');
    store.upsertManualSplit('dl-1', { ...split, revision: 6n });
    expect(useDownloadStore.getState().manualSplitById.has('dl-1')).toBe(false);
  });

  it('ignores out-of-order metrics after a terminal event', () => {
    const store = useDownloadStore.getState();
    store.upsertMetrics(metricsFor('dl-1'));
    store.removeDownload('dl-1');

    store.upsertMetrics(metricsFor('dl-1'));
    expect(useDownloadStore.getState().viewsById.has('dl-1')).toBe(false);
  });

  it('prunes expired terminated markers without touching fresh ones', () => {
    const store = useDownloadStore.getState();
    store.removeDownload('dl-1');
    expect(useDownloadStore.getState().terminatedIds.size).toBe(1);

    sweepTerminatedDownloads(Date.now());
    expect(useDownloadStore.getState().terminatedIds.size).toBe(1);

    const versionBefore = useDownloadStore.getState().version;
    sweepTerminatedDownloads(Date.now() + TERMINATED_TTL_MS + 1);
    expect(useDownloadStore.getState().terminatedIds.size).toBe(0);
    expect(useDownloadStore.getState().version).toBe(versionBefore);

    // With the marker gone, the id is usable again.
    store.upsertMetrics(metricsFor('dl-1'));
    expect(useDownloadStore.getState().viewsById.has('dl-1')).toBe(true);
  });
});

describe('useDownloadStore batched metrics', () => {
  beforeEach(() => {
    useDownloadStore.getState().clearAll();
  });

  it('applies a batch in arrival order as one update', () => {
    const store = useDownloadStore.getState();
    const versionBefore = useDownloadStore.getState().version;

    store.upsertMetricsBatch([
      { ...metricsFor('dl-1'), bytesDownloaded: 1n },
      { ...metricsFor('dl-2'), bytesDownloaded: 2n },
      { ...metricsFor('dl-1'), bytesDownloaded: 3n },
    ]);

    const state = useDownloadStore.getState();
    expect(state.version).toBe(versionBefore + 1);
    expect(state.viewsById.get('dl-1')?.bytesDownloaded).toBe(3n);
    expect(state.viewsById.get('dl-2')?.bytesDownloaded).toBe(2n);
  });

  it('skips terminated downloads and leaves the store untouched when nothing applies', () => {
    const store = useDownloadStore.getState();
    store.removeDownload('dl-1');
    const versionBefore = useDownloadStore.getState().version;

    store.upsertMetricsBatch([metricsFor('dl-1')]);

    expect(useDownloadStore.getState().viewsById.has('dl-1')).toBe(false);
    expect(useDownloadStore.getState().version).toBe(versionBefore);
  });

  it('returns the first download of a streamer', () => {
    const store = useDownloadStore.getState();
    store.upsertMeta({
      downloadId: 'dl-1',
      streamerId: 'streamer-1',
      sessionId: 'session-1',
      engineType: 'mesio',
      startedAtMs: 0n,
      updatedAtMs: 0n,
      cdnHost: '',
      downloadUrl: '',
    });

    expect(
      useDownloadStore.getState().getFirstDownloadByStreamer('streamer-1')
        ?.downloadId,
    ).toBe('dl-1');
    expect(
      useDownloadStore.getState().getFirstDownloadByStreamer('streamer-2'),
    ).toBeUndefined();
  });
});
