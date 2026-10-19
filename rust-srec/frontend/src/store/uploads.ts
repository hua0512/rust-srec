import { create } from 'zustand';

// Live upload jobs pushed over the downloads WebSocket
// (UPLOAD_STARTED / UPLOAD_PROGRESS / UPLOAD_TERMINAL / UPLOAD_QUEUE plus the
// DownloadSnapshot.uploads slice on connect/subscribe), the uploads that
// failed while this session was watching, and other pipeline jobs once they
// have run for a while (JOB_STARTED / JOB_PROGRESS / JOB_ENDED and
// DownloadSnapshot.jobs).
//
// Kept separate from useDownloadStore on purpose: that store's setSnapshot
// clears every map it owns and its `version` counter re-renders all
// subscribers, so co-locating high-frequency upload ticks there would
// thrash download consumers (and vice versa).

export interface UploadView {
  jobId: string;
  streamerId: string;
  // Empty when the job has no streamer or its name could not be resolved.
  streamerName: string;
  // Empty when the streamer has no avatar.
  streamerAvatar: string;
  sessionId: string;
  uploader: string;
  filesTotal: number;
  startedAtMs: bigint;

  // Latest progress; undefined until the first UPLOAD_PROGRESS arrives.
  percent?: number;
  bytesDone?: bigint;
  bytesTotal?: bigint;
  speedBytesPerSec?: number;
  etaSecs?: number;
  // Job-wide counters from uploaders that report them (rclone batches).
  // Separate from filesTotal, the job's input count, because progress
  // patches replace these fields wholesale.
  progressFilesDone?: number;
  progressFilesTotal?: number;

  // Wall-clock ms of the last event applied; drives the staleness guard.
  lastEventAtMs: number;
}

// A pipeline job other than an upload. The server sends only jobs that have
// been running for 10 seconds or more.
export interface JobView extends Omit<
  UploadView,
  'uploader' | 'sessionId' | 'filesTotal'
> {
  jobType: string;
  // FFmpeg steps only: output position, the length being written, and the
  // speed as a multiple of real time.
  outTimeMs?: bigint;
  durationMs?: bigint;
  speedX?: number;
}

export interface FailedUploadView extends UploadView {
  error: string;
  filesSucceeded: number;
  filesFailed: number;
  failedAtMs: number;
}

export interface UploadFailedInput {
  jobId: string;
  streamerId: string;
  error: string;
  filesSucceeded: number;
  filesFailed: number;
}

export interface UploadStartedInput {
  jobId: string;
  streamerId: string;
  streamerName: string;
  streamerAvatar: string;
  sessionId: string;
  uploader: string;
  filesTotal: number;
  startedAtMs: bigint;
}

export interface JobStartedInput extends Omit<
  UploadStartedInput,
  'uploader' | 'sessionId' | 'filesTotal'
> {
  // Job type as stored, such as "remux" or "thumbnail_hd".
  jobType: string;
}

// Also the shape of JOB_PROGRESS, which reuses the UploadProgress message.
export interface UploadProgressInput {
  jobId: string;
  streamerId: string;
  percent?: number;
  bytesDone?: bigint;
  bytesTotal?: bigint;
  speedBytesPerSec?: number;
  etaSecs?: number;
  progressFilesDone?: number;
  progressFilesTotal?: number;
  outTimeMs?: bigint;
  durationMs?: bigint;
  speedX?: number;
}

// A terminal event dropped by broadcast lag would leave a job stuck in the
// store forever; entries older than this are skipped by the selectors.
// The server repeats every running upload's state at least every 30 s, even
// while the uploader itself is silent, so a live upload never comes close
// to this threshold. Also the retention window for
// terminatedIds entries in sweepStaleUploads.
export const STALE_AFTER_MS = 2 * 60 * 1000;

// Failures are only dismissed by hand, so cap how many pile up.
const MAX_FAILED_UPLOADS = 20;

// Fills in an entry first heard of through a progress event or a failure.
const UNKNOWN_UPLOAD = {
  streamerName: '',
  streamerAvatar: '',
  sessionId: '',
  uploader: '',
  filesTotal: 0,
  startedAtMs: 0n,
};

function isFresh(view: { lastEventAtMs: number }, nowMs: number): boolean {
  return nowMs - view.lastEventAtMs < STALE_AFTER_MS;
}

function freshOldestFirst<V extends UploadView | JobView>(
  entries: Map<string, V>,
): V[] {
  const now = Date.now();
  return [...entries.values()]
    .filter((view) => isFresh(view, now))
    .sort((a, b) => Number(a.startedAtMs - b.startedAtMs));
}

/**
 * Applies progress in arrival order, skipping jobs that already ended. A job
 * without an entry (its start predates this connection and no snapshot
 * carried it) gets `unknown` filled in so it still shows. Returns whether
 * anything changed.
 */
function applyProgress<V extends UploadView | JobView>(
  entries: Map<string, V>,
  terminatedIds: Map<string, TerminatedMarker>,
  batch: UploadProgressInput[],
  unknown: Omit<V, keyof UploadProgressInput | 'lastEventAtMs'>,
): boolean {
  const now = Date.now();
  let changed = false;
  for (const progress of batch) {
    if (terminatedIds.has(progress.jobId)) continue;
    entries.set(progress.jobId, {
      ...(entries.get(progress.jobId) ?? unknown),
      ...progress,
      lastEventAtMs: now,
    } as V);
    changed = true;
  }
  return changed;
}

interface TerminatedMarker {
  atMs: number;
  // JOB_ENDED only: the ended attempt's start, as in its JOB_STARTED.
  startedAtMs?: bigint;
}

interface UploadStoreState {
  uploadsByJobId: Map<string, UploadView>;
  jobsByJobId: Map<string, JobView>;
  // Jobs that received UPLOAD_TERMINAL or JOB_ENDED, keyed to the wall-clock
  // ms of that event. Progress flows through the server's async coalescing
  // channel, so late progress can arrive after the terminal event; without this
  // guard it would resurrect the removed entry (same rationale as
  // terminatedIds in store/downloads.ts). Cleared on snapshot/clearAll;
  // entries older than STALE_AFTER_MS are dropped by sweepStaleUploads so
  // the map stays bounded across a long-lived connection.
  terminatedIds: Map<string, TerminatedMarker>;
  // Uploads that failed during this session, oldest first, kept until
  // dismissed or retried. Snapshots clear failures for running jobs and
  // preserve unrelated failures; the map is emptied on sign-out.
  failedByJobId: Map<string, FailedUploadView>;
  // Upload jobs waiting for a worker.
  pendingCount: number;
  // Bumps on any mutation; can be selected to force rerenders.
  version: number;

  // `progress` covers both uploads and jobs.
  setSnapshot: (
    uploads: UploadStartedInput[],
    progress: UploadProgressInput[],
    pendingCount?: number,
    jobs?: JobStartedInput[],
  ) => void;
  upsertStarted: (started: UploadStartedInput) => void;
  upsertProgress: (progress: UploadProgressInput) => void;
  // Applies progress in arrival order as one store update.
  upsertProgressBatch: (batch: UploadProgressInput[]) => void;
  upsertJobStarted: (started: JobStartedInput) => void;
  upsertJobProgressBatch: (batch: UploadProgressInput[]) => void;
  // Ends an upload or a job; JOB_ENDED passes the ended attempt's start.
  remove: (jobId: string, startedAtMs?: bigint) => void;
  // Like remove, but keeps the upload on screen as failed.
  fail: (failed: UploadFailedInput) => void;
  dismissFailed: (jobId: string) => void;
  setPendingCount: (pendingCount: number) => void;
  clearAll: () => void;

  getActiveUploadsByStreamer: (streamerId: string) => UploadView[];
  // Every fresh upload, oldest first.
  getActiveUploads: () => UploadView[];
  // Every fresh job, oldest first.
  getActiveJobs: () => JobView[];
  // Failed uploads, newest first.
  getFailedUploads: () => FailedUploadView[];
}

export const useUploadStore = create<UploadStoreState>((set, get) => ({
  uploadsByJobId: new Map(),
  jobsByJobId: new Map(),
  terminatedIds: new Map(),
  failedByJobId: new Map(),
  pendingCount: 0,
  version: 0,

  setSnapshot: (uploads, progress, pendingCount = 0, jobs = []) =>
    set((state) => {
      state.uploadsByJobId.clear();
      state.jobsByJobId.clear();
      state.terminatedIds.clear();
      const now = Date.now();
      for (const started of uploads) {
        // A retry may have started while the socket was disconnected.
        state.failedByJobId.delete(started.jobId);
        state.uploadsByJobId.set(started.jobId, {
          ...started,
          lastEventAtMs: now,
        });
      }
      for (const started of jobs) {
        state.jobsByJobId.set(started.jobId, {
          ...started,
          lastEventAtMs: now,
        });
      }
      for (const p of progress) {
        const existing =
          state.uploadsByJobId.get(p.jobId) ?? state.jobsByJobId.get(p.jobId);
        if (existing) {
          Object.assign(existing, p, { lastEventAtMs: now });
        }
      }
      return {
        uploadsByJobId: state.uploadsByJobId,
        jobsByJobId: state.jobsByJobId,
        terminatedIds: state.terminatedIds,
        failedByJobId: state.failedByJobId,
        pendingCount,
        version: state.version + 1,
      };
    }),

  upsertStarted: (started) =>
    set((state) => {
      // A retried job reuses its job id, and STARTED is only ever emitted
      // after the previous run's terminal event (same broadcast channel,
      // FIFO per connection) — so STARTED authoritatively un-terminates,
      // and a retry replaces the earlier failure.
      state.terminatedIds.delete(started.jobId);
      state.failedByJobId.delete(started.jobId);
      state.uploadsByJobId.set(started.jobId, {
        ...started,
        lastEventAtMs: Date.now(),
      });
      return {
        uploadsByJobId: state.uploadsByJobId,
        terminatedIds: state.terminatedIds,
        failedByJobId: state.failedByJobId,
        version: state.version + 1,
      };
    }),

  upsertProgress: (progress) => get().upsertProgressBatch([progress]),

  upsertProgressBatch: (batch) =>
    set((state) =>
      applyProgress(
        state.uploadsByJobId,
        state.terminatedIds,
        batch,
        UNKNOWN_UPLOAD,
      )
        ? { uploadsByJobId: state.uploadsByJobId, version: state.version + 1 }
        : state,
    ),

  upsertJobStarted: (started) =>
    set((state) => {
      // JOB_STARTED can trail its own attempt's JOB_ENDED. A retry reuses
      // the job id but starts later, so it un-terminates as with uploads.
      if (
        state.terminatedIds.get(started.jobId)?.startedAtMs ===
        started.startedAtMs
      ) {
        return state;
      }
      state.terminatedIds.delete(started.jobId);
      // The server also announces jobs a snapshot already delivered, so keep
      // the progress shown until the next update.
      state.jobsByJobId.set(started.jobId, {
        ...state.jobsByJobId.get(started.jobId),
        ...started,
        lastEventAtMs: Date.now(),
      });
      return {
        jobsByJobId: state.jobsByJobId,
        terminatedIds: state.terminatedIds,
        version: state.version + 1,
      };
    }),

  upsertJobProgressBatch: (batch) =>
    set((state) =>
      applyProgress(state.jobsByJobId, state.terminatedIds, batch, {
        streamerName: '',
        streamerAvatar: '',
        jobType: '',
        startedAtMs: 0n,
      })
        ? { jobsByJobId: state.jobsByJobId, version: state.version + 1 }
        : state,
    ),

  remove: (jobId, startedAtMs) =>
    set((state) => {
      state.terminatedIds.set(jobId, { atMs: Date.now(), startedAtMs });
      const removed =
        state.uploadsByJobId.delete(jobId) || state.jobsByJobId.delete(jobId);
      if (!removed) return state;
      return {
        uploadsByJobId: state.uploadsByJobId,
        jobsByJobId: state.jobsByJobId,
        terminatedIds: state.terminatedIds,
        version: state.version + 1,
      };
    }),

  fail: (failed) =>
    set((state) => {
      const now = Date.now();
      state.terminatedIds.set(failed.jobId, { atMs: now });
      const view = state.uploadsByJobId.get(failed.jobId);
      state.uploadsByJobId.delete(failed.jobId);
      // Re-inserting moves a repeat failure to the end (newest).
      state.failedByJobId.delete(failed.jobId);
      state.failedByJobId.set(failed.jobId, {
        // The failure can be the first this session hears of the job.
        ...UNKNOWN_UPLOAD,
        ...view,
        ...failed,
        lastEventAtMs: now,
        failedAtMs: now,
      });
      for (const jobId of state.failedByJobId.keys()) {
        if (state.failedByJobId.size <= MAX_FAILED_UPLOADS) break;
        state.failedByJobId.delete(jobId);
      }
      return {
        uploadsByJobId: state.uploadsByJobId,
        terminatedIds: state.terminatedIds,
        failedByJobId: state.failedByJobId,
        version: state.version + 1,
      };
    }),

  dismissFailed: (jobId) =>
    set((state) => {
      if (!state.failedByJobId.delete(jobId)) return state;
      return {
        failedByJobId: state.failedByJobId,
        version: state.version + 1,
      };
    }),

  setPendingCount: (pendingCount) =>
    set((state) =>
      state.pendingCount === pendingCount ? state : { pendingCount },
    ),

  clearAll: () =>
    set((state) => {
      state.uploadsByJobId.clear();
      state.jobsByJobId.clear();
      state.terminatedIds.clear();
      state.failedByJobId.clear();
      return {
        uploadsByJobId: state.uploadsByJobId,
        jobsByJobId: state.jobsByJobId,
        terminatedIds: state.terminatedIds,
        failedByJobId: state.failedByJobId,
        pendingCount: 0,
        version: state.version + 1,
      };
    }),

  getActiveUploadsByStreamer: (streamerId) => {
    const now = Date.now();
    const result: UploadView[] = [];
    for (const view of get().uploadsByJobId.values()) {
      if (view.streamerId === streamerId && isFresh(view, now)) {
        result.push(view);
      }
    }
    return result;
  },

  getActiveUploads: () => freshOldestFirst(get().uploadsByJobId),

  getActiveJobs: () => freshOldestFirst(get().jobsByJobId),

  getFailedUploads: () => [...get().failedByJobId.values()].reverse(),
}));

// Selectors only re-run on store mutations, so without a sweep an entry
// whose terminal event was dropped by broadcast lag would keep its badge
// (and its map slot) forever once events stop arriving. The sweep deletes
// stale entries and bumps `version` so subscribers re-render. Interval is
// coarse on purpose: STALE_AFTER_MS is minutes, cadence needn't be finer.
const STALE_SWEEP_INTERVAL_MS = 30 * 1000;

// Exported so tests can drive the sweep directly with a synthetic clock.
export function sweepStaleUploads(nowMs = Date.now()): void {
  const { uploadsByJobId, jobsByJobId, terminatedIds, version } =
    useUploadStore.getState();
  let removed = false;
  for (const entries of [uploadsByJobId, jobsByJobId]) {
    for (const [jobId, view] of entries) {
      if (!isFresh(view, nowMs)) {
        entries.delete(jobId);
        removed = true;
      }
    }
  }
  // A terminated marker only needs to outlive the server's coalescing
  // delay on late UPLOAD_PROGRESS; after STALE_AFTER_MS any progress for
  // that job would be discarded as stale anyway. No version bump —
  // nothing renders these ids.
  for (const [jobId, { atMs }] of terminatedIds) {
    if (nowMs - atMs >= STALE_AFTER_MS) {
      terminatedIds.delete(jobId);
    }
  }
  if (removed) {
    useUploadStore.setState({
      uploadsByJobId,
      jobsByJobId,
      version: version + 1,
    });
  }
}

if (typeof window !== 'undefined') {
  setInterval(() => sweepStaleUploads(), STALE_SWEEP_INTERVAL_MS);
}
