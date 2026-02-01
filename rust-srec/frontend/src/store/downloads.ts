import { create } from 'zustand';
import type { DownloadProgress } from '../api/proto/download_progress';

export type Download = DownloadProgress;

// Connection status type
export type ConnectionStatus =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'error';

interface DownloadState {
  downloads: Map<string, DownloadProgress>;
  // Bumps on any downloads Map mutation; can be selected to force rerenders.
  downloadsVersion: number;
  connectionStatus: ConnectionStatus;

  // Actions
  setSnapshot: (downloads: DownloadProgress[]) => void;
  addDownload: (download: DownloadProgress) => void;
  updateProgress: (
    downloadId: string,
    progress: Partial<DownloadProgress>,
  ) => void;
  removeDownload: (downloadId: string) => void;
  setConnectionStatus: (status: ConnectionStatus) => void;
  clearAll: () => void;

  // Selectors
  getDownloadsByStreamer: (streamerId: string) => DownloadProgress[];
  hasActiveDownload: (streamerId: string) => boolean;
}

export const useDownloadStore = create<DownloadState>((set, get) => ({
  downloads: new Map(),
  downloadsVersion: 0,
  connectionStatus: 'disconnected',

  // Note: We mutate the Map in-place to avoid allocating a new Map on every
  // progress tick. Consumers should prefer selectors like getDownloadsByStreamer
  // (which returns new arrays) rather than selecting `downloads` directly.
  setSnapshot: (downloads) =>
    set((state) => {
      state.downloads.clear();
      for (const d of downloads) {
        state.downloads.set(d.downloadId, d);
      }
      return {
        downloads: state.downloads,
        downloadsVersion: state.downloadsVersion + 1,
      };
    }),

  addDownload: (download) =>
    set((state) => {
      state.downloads.set(download.downloadId, download);
      return {
        downloads: state.downloads,
        downloadsVersion: state.downloadsVersion + 1,
      };
    }),

  updateProgress: (downloadId, progress) =>
    set((state) => {
      const existing = state.downloads.get(downloadId);
      if (!existing) return state;
      // Preserve referential updates for shallow-equality selectors.
      state.downloads.set(downloadId, { ...existing, ...progress });
      return {
        downloads: state.downloads,
        downloadsVersion: state.downloadsVersion + 1,
      };
    }),

  removeDownload: (downloadId) =>
    set((state) => {
      if (!state.downloads.has(downloadId)) return state;
      state.downloads.delete(downloadId);
      return {
        downloads: state.downloads,
        downloadsVersion: state.downloadsVersion + 1,
      };
    }),

  setConnectionStatus: (status) => set({ connectionStatus: status }),

  clearAll: () =>
    set((state) => {
      state.downloads.clear();
      return {
        downloads: state.downloads,
        downloadsVersion: state.downloadsVersion + 1,
        connectionStatus: 'disconnected',
      };
    }),

  getDownloadsByStreamer: (streamerId) => {
    return Array.from(get().downloads.values()).filter(
      (d) => d.streamerId === streamerId,
    );
  },

  hasActiveDownload: (streamerId) => {
    return get().getDownloadsByStreamer(streamerId).length > 0;
  },
}));
