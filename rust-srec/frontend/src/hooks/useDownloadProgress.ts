/**
 * WebSocket hook for real-time download progress updates.
 * Connects to /api/downloads/ws and dispatches events to the download store.
 *
 * Requirements: 1.1, 1.2, 1.3, 1.4, 2.1, 3.1, 3.2, 3.3, 3.4, 6.1, 6.2
 */
import { useEffect, useRef, useCallback } from 'react';
import { useAuthStore } from '../store/auth';
import { useDownloadStore } from '../store/downloads';
import {
  decodeWsMessage,
  encodeClientMessage,
  EventType,
  type DownloadProgress,
} from '../api/proto/download_progress';

// Reconnection constants
const WS_RECONNECT_BASE_DELAY = 1000;
const WS_RECONNECT_MAX_DELAY = 30000;

/**
 * Build the WebSocket URL with JWT token as query parameter.
 * Uses VITE_API_BASE_URL environment variable for the base URL.
 * Property 1: WebSocket URL formation - validates Requirements 1.1
 */
export function buildWebSocketUrl(accessToken: string): string {
  const apiBaseUrl = import.meta.env.VITE_API_BASE_URL || '/api';

  // Parse the base URL to extract host and path
  let wsUrl: string;

  if (apiBaseUrl.startsWith('http://') || apiBaseUrl.startsWith('https://')) {
    // Absolute URL - convert http(s) to ws(s)
    const url = new URL(apiBaseUrl);
    const wsProtocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    wsUrl = `${wsProtocol}//${url.host}${url.pathname}`;
  } else {
    // Relative URL - use current window location
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    wsUrl = `${protocol}//${window.location.host}${apiBaseUrl}`;
  }

  // Ensure path ends without trailing slash and append the WebSocket endpoint
  const basePath = wsUrl.replace(/\/$/, '');
  return `${basePath}/downloads/ws?token=${accessToken}`;
}

interface UseDownloadProgressOptions {
  /** Optional streamer ID to subscribe to for filtered updates */
  streamerId?: string;
}

/**
 * Hook to manage WebSocket connection for download progress updates.
 * Handles connection lifecycle, reconnection, and event dispatching.
 */
export function useDownloadProgress(options: UseDownloadProgressOptions = {}) {
  const { streamerId } = options;

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptRef = useRef<number>(0);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const isConnectingRef = useRef<boolean>(false);

  // Auth state
  const accessToken = useAuthStore((state) => state.accessToken);
  const isAuthenticated = useAuthStore((state) => state.isAuthenticated);

  // Download store actions
  const setSnapshot = useDownloadStore((state) => state.setSnapshot);
  const addDownload = useDownloadStore((state) => state.addDownload);
  const updateProgress = useDownloadStore((state) => state.updateProgress);
  const removeDownload = useDownloadStore((state) => state.removeDownload);
  const setConnectionStatus = useDownloadStore((state) => state.setConnectionStatus);
  const clearAll = useDownloadStore((state) => state.clearAll);


  /**
   * Handle incoming WebSocket messages.
   * Decodes protobuf and dispatches to appropriate store actions.
   * Requirements: 2.1, 3.1, 3.2, 3.3, 3.4
   */
  const handleMessage = useCallback(
    (event: MessageEvent) => {
      try {
        const data = new Uint8Array(event.data as ArrayBuffer);
        const message = decodeWsMessage(data);
        console.debug('[WS] Received message:', message.eventType);

        switch (message.eventType) {
          case EventType.EVENT_TYPE_SNAPSHOT:
            if ('snapshot' in message.payload) {
              setSnapshot(message.payload.snapshot.downloads);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_STARTED:
            if ('downloadStarted' in message.payload) {
              const started = message.payload.downloadStarted;
              // Convert DownloadStarted to DownloadProgress with initial values
              const initialProgress: DownloadProgress = {
                downloadId: started.downloadId,
                streamerId: started.streamerId,
                sessionId: started.sessionId,
                engineType: started.engineType,
                status: 'Starting',
                bytesDownloaded: 0n,
                durationSecs: 0,
                speedBytesPerSec: 0n,
                segmentsCompleted: 0,
                mediaDurationSecs: 0,
                playbackRatio: 0,
                startedAtMs: started.startedAtMs,
              };
              addDownload(initialProgress);
            }
            break;

          case EventType.EVENT_TYPE_PROGRESS:
            if ('progress' in message.payload) {
              const progress = message.payload.progress;
              updateProgress(progress.downloadId, progress);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_COMPLETED:
            if ('downloadCompleted' in message.payload) {
              removeDownload(message.payload.downloadCompleted.downloadId);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_FAILED:
            if ('downloadFailed' in message.payload) {
              removeDownload(message.payload.downloadFailed.downloadId);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_CANCELLED:
            if ('downloadCancelled' in message.payload) {
              removeDownload(message.payload.downloadCancelled.downloadId);
            }
            break;

          case EventType.EVENT_TYPE_SEGMENT_COMPLETED:
            // Segment completed events are informational; progress updates handle the state
            break;

          case EventType.EVENT_TYPE_ERROR:
            if ('error' in message.payload) {
              console.error('WebSocket error from server:', message.payload.error);
            }
            break;

          default:
            console.warn('Unknown event type:', message.eventType);
        }
      } catch (error) {
        // Requirement 2.3: Log decode errors and continue processing
        console.error('Failed to decode WebSocket message:', error);
      }
    },
    [setSnapshot, addDownload, updateProgress, removeDownload]
  );


  /**
   * Schedule a reconnection attempt with exponential backoff.
   * Requirement 1.3: Reconnect with exponential backoff
   */
  const scheduleReconnect = useCallback(() => {
    const delay = Math.min(
      WS_RECONNECT_BASE_DELAY * Math.pow(2, reconnectAttemptRef.current),
      WS_RECONNECT_MAX_DELAY
    );
    reconnectAttemptRef.current++;

    reconnectTimeoutRef.current = setTimeout(() => {
      connect();
    }, delay);
  }, []);

  /**
   * Establish WebSocket connection.
   * Requirement 1.1: Connect with JWT token as query parameter
   * Requirement 1.2: Receive and decode initial snapshot
   */
  const connect = useCallback(() => {
    if (!accessToken || !isAuthenticated) return;
    if (isConnectingRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    isConnectingRef.current = true;
    setConnectionStatus('connecting');

    const wsUrl = buildWebSocketUrl(accessToken);
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      console.debug('[WS] Connected');
      isConnectingRef.current = false;
      setConnectionStatus('connected');
      reconnectAttemptRef.current = 0; // Reset reconnect attempts on success

      // Send subscribe message if streamerId is provided (Requirement 6.1)
      if (streamerId) {
        const msg = encodeClientMessage({
          action: { subscribe: { streamerId } },
        });
        ws.send(msg);
      }
    };

    ws.onmessage = handleMessage;

    ws.onclose = () => {
      console.debug('[WS] Disconnected');
      isConnectingRef.current = false;
      setConnectionStatus('disconnected');
      wsRef.current = null;

      // Only reconnect if still authenticated
      if (useAuthStore.getState().isAuthenticated) {
        scheduleReconnect();
      }
    };

    ws.onerror = () => {
      console.error('[WS] Connection error');
      isConnectingRef.current = false;
      setConnectionStatus('error');
    };

    wsRef.current = ws;
  }, [accessToken, isAuthenticated, streamerId, handleMessage, setConnectionStatus, scheduleReconnect]);

  /**
   * Disconnect and cleanup.
   * Requirement 1.4: Close connection and clear state on logout
   */
  const disconnect = useCallback(() => {
    // Clear any pending reconnect
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = undefined;
    }

    // Close WebSocket
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    isConnectingRef.current = false;
    clearAll();
  }, [clearAll]);


  // Connect on mount when authenticated, disconnect on unmount
  useEffect(() => {
    if (isAuthenticated && accessToken) {
      connect();
    } else {
      // Requirement 1.4: Clear state when not authenticated
      disconnect();
    }

    return () => {
      disconnect();
    };
  }, [isAuthenticated, accessToken, connect, disconnect]);

  // Handle streamer subscription changes (Requirements 6.1, 6.2)
  useEffect(() => {
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;

    if (streamerId) {
      // Subscribe to specific streamer
      const msg = encodeClientMessage({
        action: { subscribe: { streamerId } },
      });
      ws.send(msg);
    } else {
      // Unsubscribe when streamerId becomes undefined
      const msg = encodeClientMessage({
        action: { unsubscribe: {} },
      });
      ws.send(msg);
    }
  }, [streamerId]);

  // Listen to auth state changes for logout cleanup (Requirement 1.4)
  useEffect(() => {
    const unsubscribe = useAuthStore.subscribe((state, prevState) => {
      // If user logged out, disconnect
      if (prevState.isAuthenticated && !state.isAuthenticated) {
        disconnect();
      }
    });

    return () => {
      unsubscribe();
    };
  }, [disconnect]);

  return { disconnect };
}
