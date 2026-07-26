import { STALE_AFTER_MS, sweepStaleUploads, useUploadStore } from '../uploads';

function startedInput(jobId: string) {
  return {
    jobId,
    streamerId: 'streamer-1',
    sessionId: 'session-1',
    uploader: 'rclone',
    filesTotal: 1,
    startedAtMs: 0n,
  };
}

describe('useUploadStore', () => {
  beforeEach(() => {
    useUploadStore.getState().clearAll();
  });

  it('ignores progress for a terminated job until a new STARTED arrives', () => {
    const store = useUploadStore.getState();
    store.upsertStarted(startedInput('job-1'));
    store.remove('job-1');

    store.upsertProgress({
      jobId: 'job-1',
      streamerId: 'streamer-1',
      percent: 50,
    });
    expect(useUploadStore.getState().uploadsByJobId.has('job-1')).toBe(false);

    store.upsertStarted(startedInput('job-1'));
    store.upsertProgress({
      jobId: 'job-1',
      streamerId: 'streamer-1',
      percent: 50,
    });
    expect(useUploadStore.getState().uploadsByJobId.get('job-1')?.percent).toBe(
      50,
    );
  });

  it('sweeps stale upload views together with expired terminated markers', () => {
    const store = useUploadStore.getState();
    store.upsertStarted(startedInput('job-live'));
    store.upsertStarted(startedInput('job-done'));
    store.remove('job-done');

    sweepStaleUploads(Date.now() + STALE_AFTER_MS + 1);

    const state = useUploadStore.getState();
    expect(state.uploadsByJobId.size).toBe(0);
    expect(state.terminatedIds.size).toBe(0);
  });

  it('prunes terminated markers even when no upload views remain', () => {
    const store = useUploadStore.getState();
    store.upsertStarted(startedInput('job-1'));
    store.remove('job-1');
    expect(useUploadStore.getState().uploadsByJobId.size).toBe(0);
    expect(useUploadStore.getState().terminatedIds.size).toBe(1);

    // A fresh marker survives: it still guards against late progress.
    sweepStaleUploads(Date.now());
    expect(useUploadStore.getState().terminatedIds.size).toBe(1);

    // An expired marker is dropped without a version bump — the sweep must
    // not force re-renders for state nothing subscribes to.
    const versionBefore = useUploadStore.getState().version;
    sweepStaleUploads(Date.now() + STALE_AFTER_MS + 1);
    expect(useUploadStore.getState().terminatedIds.size).toBe(0);
    expect(useUploadStore.getState().version).toBe(versionBefore);
  });
});
