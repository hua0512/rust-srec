import { create } from 'zustand';

// Live upload jobs pushed over the downloads WebSocket
// (UPLOAD_STARTED / UPLOAD_PROGRESS / UPLOAD_TERMINAL plus the
// DownloadSnapshot.uploads slice on connect/subscribe).
//
// Kept separate from useDownloadStore on purpose: that store's setSnapshot
// clears every map it owns and its `version` counter re-renders all
// subscribers, so co-locating high-frequency upload ticks there would
// thrash download consumers (and vice versa).

export interface UploadView {
  jobId: string;
  streamerId: string;
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

  // Wall-clock ms of the last event applied; drives the staleness guard.
  lastEventAtMs: number;
}

export interface UploadStartedInput {
  jobId: string;
  streamerId: string;
  sessionId: string;
  uploader: string;
  filesTotal: number;
  startedAtMs: bigint;
}

export interface UploadProgressInput {
  jobId: string;
  streamerId: string;
  percent?: number;
  bytesDone?: bigint;
  bytesTotal?: bigint;
  speedBytesPerSec?: number;
  etaSecs?: number;
}

// A terminal event dropped by broadcast lag would leave a job stuck in the
// store forever; entries older than this are skipped by the selectors.
// Rclone reports stats every second while transferring, so a live upload
// never comes close to this threshold.
const STALE_AFTER_MS = 2 * 60 * 1000;

function isFresh(view: UploadView, nowMs: number): boolean {
  return nowMs - view.lastEventAtMs < STALE_AFTER_MS;
}

interface UploadStoreState {
  uploadsByJobId: Map<string, UploadView>;
  // Jobs that received UPLOAD_TERMINAL. Progress flows through the server's
  // async coalescing channel, so a late UPLOAD_PROGRESS can arrive after the
  // terminal event; without this guard it would resurrect the removed entry
  // (same rationale as terminatedIds in store/downloads.ts). Cleared on
  // snapshot/clearAll.
  terminatedIds: Set<string>;
  // Bumps on any mutation; can be selected to force rerenders.
  version: number;

  setSnapshot: (
    uploads: UploadStartedInput[],
    progress: UploadProgressInput[],
  ) => void;
  upsertStarted: (started: UploadStartedInput) => void;
  upsertProgress: (progress: UploadProgressInput) => void;
  remove: (jobId: string) => void;
  clearAll: () => void;

  getActiveUploadsByStreamer: (streamerId: string) => UploadView[];
}

export const useUploadStore = create<UploadStoreState>((set, get) => ({
  uploadsByJobId: new Map(),
  terminatedIds: new Set(),
  version: 0,

  setSnapshot: (uploads, progress) =>
    set((state) => {
      state.uploadsByJobId.clear();
      state.terminatedIds.clear();
      const now = Date.now();
      for (const started of uploads) {
        state.uploadsByJobId.set(started.jobId, {
          ...started,
          lastEventAtMs: now,
        });
      }
      for (const p of progress) {
        const existing = state.uploadsByJobId.get(p.jobId);
        if (existing) {
          Object.assign(existing, p, { lastEventAtMs: now });
        }
      }
      return {
        uploadsByJobId: state.uploadsByJobId,
        terminatedIds: state.terminatedIds,
        version: state.version + 1,
      };
    }),

  upsertStarted: (started) =>
    set((state) => {
      // A retried job reuses its job id, and STARTED is only ever emitted
      // after the previous run's terminal event (same broadcast channel,
      // FIFO per connection) — so STARTED authoritatively un-terminates.
      state.terminatedIds.delete(started.jobId);
      state.uploadsByJobId.set(started.jobId, {
        ...started,
        lastEventAtMs: Date.now(),
      });
      return {
        uploadsByJobId: state.uploadsByJobId,
        terminatedIds: state.terminatedIds,
        version: state.version + 1,
      };
    }),

  upsertProgress: (progress) =>
    set((state) => {
      if (state.terminatedIds.has(progress.jobId)) return state;
      const existing = state.uploadsByJobId.get(progress.jobId);
      if (existing) {
        state.uploadsByJobId.set(progress.jobId, {
          ...existing,
          ...progress,
          lastEventAtMs: Date.now(),
        });
      } else {
        // Progress for an unknown job (its STARTED event predates this
        // connection and no snapshot carried it, e.g. a subscribe-filter
        // race). Synthesize a minimal entry so the indicator still shows.
        state.uploadsByJobId.set(progress.jobId, {
          sessionId: '',
          uploader: '',
          filesTotal: 0,
          startedAtMs: 0n,
          ...progress,
          lastEventAtMs: Date.now(),
        });
      }
      return {
        uploadsByJobId: state.uploadsByJobId,
        version: state.version + 1,
      };
    }),

  remove: (jobId) =>
    set((state) => {
      state.terminatedIds.add(jobId);
      if (!state.uploadsByJobId.delete(jobId)) return state;
      return {
        uploadsByJobId: state.uploadsByJobId,
        terminatedIds: state.terminatedIds,
        version: state.version + 1,
      };
    }),

  clearAll: () =>
    set((state) => {
      state.uploadsByJobId.clear();
      state.terminatedIds.clear();
      return {
        uploadsByJobId: state.uploadsByJobId,
        terminatedIds: state.terminatedIds,
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
}));

// Selectors only re-run on store mutations, so without a sweep an entry
// whose terminal event was dropped by broadcast lag would keep its badge
// (and its map slot) forever once events stop arriving. The sweep deletes
// stale entries and bumps `version` so subscribers re-render. Interval is
// coarse on purpose: STALE_AFTER_MS is minutes, cadence needn't be finer.
const STALE_SWEEP_INTERVAL_MS = 30 * 1000;

if (typeof window !== 'undefined') {
  setInterval(() => {
    const { uploadsByJobId, version } = useUploadStore.getState();
    if (uploadsByJobId.size === 0) return;
    const now = Date.now();
    let removed = false;
    for (const [jobId, view] of uploadsByJobId) {
      if (!isFresh(view, now)) {
        uploadsByJobId.delete(jobId);
        removed = true;
      }
    }
    if (removed) {
      useUploadStore.setState({ uploadsByJobId, version: version + 1 });
    }
  }, STALE_SWEEP_INTERVAL_MS);
}
