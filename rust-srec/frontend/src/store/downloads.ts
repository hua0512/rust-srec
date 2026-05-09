import { create } from 'zustand';

// Connection status type
export type ConnectionStatus =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'error';

// Plain object versions of proto types for store state
export interface DownloadMeta {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  engineType: string;
  startedAtMs: bigint;
  updatedAtMs: bigint;
  cdnHost: string;
  downloadUrl: string;
}

export interface DownloadMetrics {
  downloadId: string;
  status: string;
  bytesDownloaded: bigint;
  durationSecs: number;
  speedBytesPerSec: bigint;
  segmentsCompleted: number;
  mediaDurationSecs: number;
  playbackRatio: number;
}

export interface DownloadState {
  meta: DownloadMeta;
  metrics: DownloadMetrics;
}

export interface DownloadView {
  downloadId: string;

  // Meta
  streamerId: string;
  sessionId: string;
  engineType: string;
  startedAtMs: bigint;
  updatedAtMs: bigint;
  cdnHost: string;
  downloadUrl: string;

  // Metrics
  status: string;
  bytesDownloaded: bigint;
  durationSecs: number;
  speedBytesPerSec: bigint;
  segmentsCompleted: number;
  mediaDurationSecs: number;
  playbackRatio: number;
}

export type Download = DownloadView;

// Streamers parked on the concurrency queue: live but waiting for a
// download slot. Surfaced on the streamer card as a "Queued" badge.
export interface QueuedEntry {
  streamerId: string;
  sessionId: string;
  streamerName: string;
  engineType: string;
  queuedAtMs: bigint;
  isHighPriority: boolean;
}

function emptyMeta(downloadId: string): DownloadMeta {
  return {
    downloadId,
    streamerId: '',
    sessionId: '',
    engineType: '',
    startedAtMs: 0n,
    updatedAtMs: 0n,
    cdnHost: '',
    downloadUrl: '',
  };
}

function emptyMetrics(downloadId: string): DownloadMetrics {
  return {
    downloadId,
    status: '',
    bytesDownloaded: 0n,
    durationSecs: 0,
    speedBytesPerSec: 0n,
    segmentsCompleted: 0,
    mediaDurationSecs: 0,
    playbackRatio: 0,
  };
}

function toView(meta: DownloadMeta, metrics: DownloadMetrics): DownloadView {
  const downloadId = meta.downloadId || metrics.downloadId;
  return {
    downloadId,
    streamerId: meta.streamerId,
    sessionId: meta.sessionId,
    engineType: meta.engineType,
    startedAtMs: meta.startedAtMs,
    updatedAtMs: meta.updatedAtMs,
    cdnHost: meta.cdnHost,
    downloadUrl: meta.downloadUrl,

    status: metrics.status,
    bytesDownloaded: metrics.bytesDownloaded,
    durationSecs: metrics.durationSecs,
    speedBytesPerSec: metrics.speedBytesPerSec,
    segmentsCompleted: metrics.segmentsCompleted,
    mediaDurationSecs: metrics.mediaDurationSecs,
    playbackRatio: metrics.playbackRatio,
  };
}

interface DownloadStoreState {
  metaById: Map<string, DownloadMeta>;
  metricsById: Map<string, DownloadMetrics>;
  viewsById: Map<string, DownloadView>;
  // Download IDs that have received a terminal event.
  // Used to ignore out-of-order metrics/meta that arrive after termination.
  terminatedIds: Set<string>;
  // Streamers waiting for a download slot. Keyed by streamerId so the
  // card lookup is O(1). At most one entry per streamer (the queue
  // itself dedupes by session_id, and the streamer card only renders
  // one badge).
  queuedByStreamer: Map<string, QueuedEntry>;
  // Bumps on any mutation; can be selected to force rerenders.
  version: number;
  connectionStatus: ConnectionStatus;

  // Actions
  setSnapshot: (downloads: DownloadState[], queued: QueuedEntry[]) => void;
  upsertMeta: (meta: DownloadMeta) => void;
  upsertMetrics: (metrics: DownloadMetrics) => void;
  removeDownload: (downloadId: string) => void;
  setQueued: (entry: QueuedEntry) => void;
  clearQueuedByStreamer: (streamerId: string) => void;
  setConnectionStatus: (status: ConnectionStatus) => void;
  clearAll: () => void;

  // Selectors
  getDownloadsByStreamer: (streamerId: string) => DownloadView[];
  hasActiveDownload: (streamerId: string) => boolean;
  getQueuedForStreamer: (streamerId: string) => QueuedEntry | undefined;
}

export const useDownloadStore = create<DownloadStoreState>((set, get) => ({
  metaById: new Map(),
  metricsById: new Map(),
  viewsById: new Map(),
  terminatedIds: new Set(),
  queuedByStreamer: new Map(),
  version: 0,
  connectionStatus: 'disconnected',

  setSnapshot: (downloads, queued) =>
    set((state) => {
      state.metaById.clear();
      state.metricsById.clear();
      state.viewsById.clear();
      state.terminatedIds.clear();
      state.queuedByStreamer.clear();
      for (const d of downloads) {
        const id = d.meta.downloadId || d.metrics.downloadId;
        state.metaById.set(id, d.meta);
        state.metricsById.set(id, d.metrics);
        state.viewsById.set(id, toView(d.meta, d.metrics));
      }
      for (const q of queued) {
        state.queuedByStreamer.set(q.streamerId, q);
      }
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
      };
    }),

  upsertMeta: (meta) =>
    set((state) => {
      const id = meta.downloadId;
      if (state.terminatedIds.has(id)) return state;

      const existing = state.metaById.get(id);

      // Coalesce out-of-order meta updates.
      // If server doesn't provide updatedAtMs (0), we can't order, but avoid
      // overriding versioned meta with unversioned updates.
      const incomingUpdatedAtMs = meta.updatedAtMs;
      const existingUpdatedAtMs = existing?.updatedAtMs ?? 0n;
      if (
        incomingUpdatedAtMs !== 0n &&
        existingUpdatedAtMs !== 0n &&
        incomingUpdatedAtMs < existingUpdatedAtMs
      ) {
        return state;
      }
      if (incomingUpdatedAtMs === 0n && existingUpdatedAtMs !== 0n) {
        return state;
      }

      const newMeta = { ...(existing ?? emptyMeta(id)), ...meta };
      state.metaById.set(id, newMeta);

      // Ensure metrics exists for join selectors.
      if (!state.metricsById.has(id)) {
        state.metricsById.set(id, emptyMetrics(id));
      }

      // Update view
      const metrics = state.metricsById.get(id)!;
      state.viewsById.set(id, toView(newMeta, metrics));

      // Receiving meta means the download has a real id and is no
      // longer queued; defensively clear the queued badge for this
      // streamer so a stale entry from a missed event doesn't linger.
      state.queuedByStreamer.delete(newMeta.streamerId);

      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
      };
    }),

  upsertMetrics: (metrics) =>
    set((state) => {
      const id = metrics.downloadId;
      if (state.terminatedIds.has(id)) return state;

      const existing = state.metricsById.get(id);
      const newMetrics = { ...(existing ?? emptyMetrics(id)), ...metrics };
      state.metricsById.set(id, newMetrics);

      // Ensure meta exists for join selectors.
      if (!state.metaById.has(id)) {
        state.metaById.set(id, emptyMeta(id));
      }

      // Update view
      const meta = state.metaById.get(id)!;
      state.viewsById.set(id, toView(meta, newMetrics));

      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
      };
    }),

  removeDownload: (downloadId) =>
    set((state) => {
      state.terminatedIds.add(downloadId);
      const had =
        state.metaById.delete(downloadId) ||
        state.metricsById.delete(downloadId);
      if (!had) return state;
      state.metricsById.delete(downloadId);
      state.viewsById.delete(downloadId);
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
      };
    }),

  setQueued: (entry) =>
    set((state) => {
      state.queuedByStreamer.set(entry.streamerId, entry);
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
      };
    }),

  clearQueuedByStreamer: (streamerId) =>
    set((state) => {
      if (!state.queuedByStreamer.delete(streamerId)) return state;
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
      };
    }),

  setConnectionStatus: (status) => set({ connectionStatus: status }),

  clearAll: () =>
    set((state) => {
      state.metaById.clear();
      state.metricsById.clear();
      state.viewsById.clear();
      state.terminatedIds.clear();
      state.queuedByStreamer.clear();
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
        queuedByStreamer: state.queuedByStreamer,
        version: state.version + 1,
        connectionStatus: 'disconnected',
      };
    }),

  getDownloadsByStreamer: (streamerId) => {
    const viewsById = get().viewsById;
    const result: DownloadView[] = [];
    for (const view of viewsById.values()) {
      if (view.streamerId === streamerId) {
        result.push(view);
      }
    }
    return result;
  },

  hasActiveDownload: (streamerId) =>
    get().getDownloadsByStreamer(streamerId).length > 0,

  getQueuedForStreamer: (streamerId) => get().queuedByStreamer.get(streamerId),
}));
