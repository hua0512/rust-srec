import { STALE_AFTER_MS, sweepStaleUploads, useUploadStore } from '../uploads';

function startedInput(jobId: string) {
  return {
    jobId,
    streamerId: 'streamer-1',
    streamerName: 'Streamer One',
    streamerAvatar: '',
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

describe('useUploadStore failures', () => {
  beforeEach(() => {
    useUploadStore.getState().clearAll();
  });

  it('clears failures for retries restored by a snapshot while preserving other failures', () => {
    const store = useUploadStore.getState();
    for (const jobId of ['retried-job', 'failed-job']) {
      store.upsertStarted(startedInput(jobId));
      store.fail({
        jobId,
        streamerId: 'streamer-1',
        error: 'quota exceeded',
        filesSucceeded: 0,
        filesFailed: 1,
      });
    }
    const unrelatedFailure = store
      .getFailedUploads()
      .find((upload) => upload.jobId === 'failed-job');

    // The retry's STARTED event was missed during a disconnect.
    store.setSnapshot(
      [{ ...startedInput('retried-job'), startedAtMs: 1n }],
      [{ jobId: 'retried-job', streamerId: 'streamer-1', percent: 50 }],
    );

    expect(store.getActiveUploads()).toMatchObject([
      { jobId: 'retried-job', startedAtMs: 1n, percent: 50 },
    ]);
    expect(store.getFailedUploads()).toEqual([unrelatedFailure]);

    // Successful completion must not leave the earlier failure visible.
    store.remove('retried-job');
    expect(store.getActiveUploads()).toEqual([]);
    expect(store.getFailedUploads()).toEqual([unrelatedFailure]);
  });

  it('keeps a failure through reconnect snapshots until the job is retried', () => {
    const store = useUploadStore.getState();
    store.upsertStarted(startedInput('job-1'));
    store.fail({
      jobId: 'job-1',
      streamerId: 'streamer-1',
      error: 'quota exceeded',
      filesSucceeded: 0,
      filesFailed: 1,
    });

    // A reconnect replays only running jobs.
    store.setSnapshot([], [], 3);
    const [failed] = useUploadStore.getState().getFailedUploads();
    expect(failed).toMatchObject({
      jobId: 'job-1',
      streamerName: 'Streamer One',
      streamerAvatar: '',
      error: 'quota exceeded',
    });
    expect(useUploadStore.getState().pendingCount).toBe(3);

    store.upsertStarted(startedInput('job-1'));
    expect(useUploadStore.getState().getFailedUploads()).toEqual([]);
    expect(useUploadStore.getState().uploadsByJobId.has('job-1')).toBe(true);
  });
});

describe('useUploadStore batched progress', () => {
  beforeEach(() => {
    useUploadStore.getState().clearAll();
  });

  it('applies a batch in arrival order as one update, skipping terminated jobs', () => {
    const store = useUploadStore.getState();
    store.upsertStarted(startedInput('job-1'));
    store.upsertStarted(startedInput('job-2'));
    store.remove('job-2');
    const versionBefore = useUploadStore.getState().version;

    store.upsertProgressBatch([
      { jobId: 'job-1', streamerId: 'streamer-1', percent: 10 },
      { jobId: 'job-2', streamerId: 'streamer-1', percent: 20 },
      { jobId: 'job-1', streamerId: 'streamer-1', percent: 30 },
    ]);

    const state = useUploadStore.getState();
    expect(state.version).toBe(versionBefore + 1);
    expect(state.uploadsByJobId.get('job-1')?.percent).toBe(30);
    expect(state.uploadsByJobId.has('job-2')).toBe(false);
  });
});
