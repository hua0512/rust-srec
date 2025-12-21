import { useEffect, useRef } from 'react';
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

  // Track previous streamer ID to handle changes
  const prevStreamerId = useRef<string | undefined>(undefined);

  // Handle subscription changes
  useEffect(() => {
    // If we have a new streamer ID and it's different from the previous one
    if (streamerId && streamerId !== prevStreamerId.current) {
      // Unsubscribe from previous if existed
      if (prevStreamerId.current) {
        unsubscribe(prevStreamerId.current);
      }

      // Subscribe to new
      subscribe(streamerId);
      prevStreamerId.current = streamerId;
    }
    // If streamerId became undefined but we had one before
    else if (!streamerId && prevStreamerId.current) {
      unsubscribe(prevStreamerId.current);
      prevStreamerId.current = undefined;
    }

    return () => {
      // Cleanup on unmount or change
      if (prevStreamerId.current) {
        unsubscribe(prevStreamerId.current);
      }
    };
  }, [streamerId, subscribe, unsubscribe]);

  // Hook doesn't need to return anything for now, or could return status
  return {};
}
