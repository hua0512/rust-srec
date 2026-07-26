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
