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
  // Bumps on any mutation; can be selected to force rerenders.
  version: number;
  connectionStatus: ConnectionStatus;

  // Actions
  setSnapshot: (downloads: DownloadState[]) => void;
  upsertMeta: (meta: DownloadMeta) => void;
  upsertMetrics: (metrics: DownloadMetrics) => void;
  removeDownload: (downloadId: string) => void;
  setConnectionStatus: (status: ConnectionStatus) => void;
  clearAll: () => void;

  // Selectors
  getDownloadsByStreamer: (streamerId: string) => DownloadView[];
  hasActiveDownload: (streamerId: string) => boolean;
}

export const useDownloadStore = create<DownloadStoreState>((set, get) => ({
  metaById: new Map(),
  metricsById: new Map(),
  viewsById: new Map(),
  terminatedIds: new Set(),
  version: 0,
  connectionStatus: 'disconnected',

  setSnapshot: (downloads) =>
    set((state) => {
      state.metaById.clear();
      state.metricsById.clear();
      state.viewsById.clear();
      state.terminatedIds.clear();
      for (const d of downloads) {
        const id = d.meta.downloadId || d.metrics.downloadId;
        state.metaById.set(id, d.meta);
        state.metricsById.set(id, d.metrics);
        state.viewsById.set(id, toView(d.meta, d.metrics));
      }
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
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

      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
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
      return {
        metaById: state.metaById,
        metricsById: state.metricsById,
        viewsById: state.viewsById,
        terminatedIds: state.terminatedIds,
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
}));
