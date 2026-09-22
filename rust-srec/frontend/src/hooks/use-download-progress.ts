import { useEffect } from 'react';
import { useWebSocket } from '@/providers/WebSocketContext';

interface UseDownloadProgressOptions {
  /** Optional streamer ID to subscribe to for filtered updates */
  streamerId?: string;
}

/**
 * Hook to manage WebSocket subscription for download progress updates.
 * Consumes the WebSocketContext to ensure a single connection.
 */
export function useDownloadProgress(options: UseDownloadProgressOptions = {}) {
  const { streamerId } = options;
  const { subscribe, unsubscribe } = useWebSocket();

  useEffect(() => {
    if (!streamerId) return;
    subscribe(streamerId);
    return () => unsubscribe(streamerId);
  }, [streamerId, subscribe, unsubscribe]);
}
