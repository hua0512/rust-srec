import { create } from 'zustand';
import type { DownloadProgress } from '../api/proto/download_progress';

// Connection status type
export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error';

interface DownloadState {
  downloads: Map<string, DownloadProgress>;
  connectionStatus: ConnectionStatus;

  // Actions
  setSnapshot: (downloads: DownloadProgress[]) => void;
  addDownload: (download: DownloadProgress) => void;
  updateProgress: (downloadId: string, progress: Partial<DownloadProgress>) => void;
  removeDownload: (downloadId: string) => void;
  setConnectionStatus: (status: ConnectionStatus) => void;
  clearAll: () => void;

  // Selectors
  getDownloadsByStreamer: (streamerId: string) => DownloadProgress[];
  hasActiveDownload: (streamerId: string) => boolean;
}

export const useDownloadStore = create<DownloadState>((set, get) => ({
  downloads: new Map(),
  connectionStatus: 'disconnected',

  setSnapshot: (downloads) =>
    set({
      downloads: new Map(downloads.map((d) => [d.downloadId, d])),
    }),

  addDownload: (download) =>
    set((state) => {
      const newDownloads = new Map(state.downloads);
      newDownloads.set(download.downloadId, download);
      return { downloads: newDownloads };
    }),

  updateProgress: (downloadId, progress) =>
    set((state) => {
      const existing = state.downloads.get(downloadId);
      if (!existing) return state;
      const newDownloads = new Map(state.downloads);
      newDownloads.set(downloadId, { ...existing, ...progress });
      return { downloads: newDownloads };
    }),

  removeDownload: (downloadId) =>
    set((state) => {
      const newDownloads = new Map(state.downloads);
      newDownloads.delete(downloadId);
      return { downloads: newDownloads };
    }),

  setConnectionStatus: (status) => set({ connectionStatus: status }),

  clearAll: () => set({ downloads: new Map(), connectionStatus: 'disconnected' }),

  getDownloadsByStreamer: (streamerId) => {
    return Array.from(get().downloads.values()).filter((d) => d.streamerId === streamerId);
  },

  hasActiveDownload: (streamerId) => {
    return get().getDownloadsByStreamer(streamerId).length > 0;
  },
}));
