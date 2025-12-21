import {
  useEffect,
  useRef,
  useCallback,
  ReactNode,
} from 'react';
import { useQuery } from '@tanstack/react-query';
import { sessionQueryOptions } from '@/api/session';
import { useDownloadStore } from '@/store/downloads';
import {
  decodeWsMessage,
  encodeClientMessage,
  EventType,
  type DownloadProgress,
} from '@/api/proto/download_progress';
import { buildWebSocketUrl } from '@/lib/url';
import { WebSocketContext } from './WebSocketContext';

// Reconnection constants
const WS_RECONNECT_BASE_DELAY = 1000;
const WS_RECONNECT_MAX_DELAY = 30000;

export function WebSocketProvider({ children }: { children: ReactNode }) {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptRef = useRef<number>(0);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const isConnectingRef = useRef<boolean>(false);

  // Auth state
  const { data: sessionData } = useQuery(sessionQueryOptions);
  const accessToken = sessionData?.token?.access_token;
  const isAuthenticated = !!accessToken;

  // Download store actions
  const setSnapshot = useDownloadStore((state) => state.setSnapshot);
  const addDownload = useDownloadStore((state) => state.addDownload);
  const updateProgress = useDownloadStore((state) => state.updateProgress);
  const removeDownload = useDownloadStore((state) => state.removeDownload);
  const setConnectionStatus = useDownloadStore(
    (state) => state.setConnectionStatus,
  );
  const clearAll = useDownloadStore((state) => state.clearAll);

  const handleMessage = useCallback(
    (event: MessageEvent) => {
      try {
        const data = new Uint8Array(event.data as ArrayBuffer);
        const message = decodeWsMessage(data);
        // console.debug('[WS] Received message:', message.eventType);

        switch (message.eventType) {
          case EventType.EVENT_TYPE_SNAPSHOT:
            if ('snapshot' in message.payload) {
              setSnapshot(message.payload.snapshot.downloads);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_STARTED:
            if ('downloadStarted' in message.payload) {
              const started = message.payload.downloadStarted;
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
            break;

          case EventType.EVENT_TYPE_ERROR:
            if ('error' in message.payload) {
              console.error(
                'WebSocket error from server:',
                message.payload.error,
              );
            }
            break;
        }
      } catch (error) {
        console.error('Failed to decode WebSocket message:', error);
      }
    },
    [setSnapshot, addDownload, updateProgress, removeDownload],
  );

  const connect = useCallback(() => {
    if (!accessToken || !isAuthenticated) return;
    if (isConnectingRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;
    if (wsRef.current?.readyState === WebSocket.CONNECTING) return;

    isConnectingRef.current = true;
    setConnectionStatus('connecting');

    const wsUrl = buildWebSocketUrl(accessToken);
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      console.debug('[WS] Connected');
      isConnectingRef.current = false;
      setConnectionStatus('connected');
      reconnectAttemptRef.current = 0;

      // Explicitly Clear any filters to ensure we receive everything
      const msg = encodeClientMessage({
        action: { unsubscribe: {} },
      });
      ws.send(msg);
    };

    ws.onmessage = handleMessage;

    ws.onclose = () => {
      console.debug('[WS] Disconnected');
      isConnectingRef.current = false;
      setConnectionStatus('disconnected');
      wsRef.current = null;

      if (sessionData?.token?.access_token) {
        scheduleReconnect();
      }
    };

    ws.onerror = () => {
      console.error('[WS] Connection error');
      isConnectingRef.current = false;
      setConnectionStatus('error');
    };

    wsRef.current = ws;
  }, [
    accessToken,
    isAuthenticated,
    handleMessage,
    setConnectionStatus,
    sessionData?.token?.access_token,
  ]);

  const scheduleReconnect = useCallback(() => {
    const delay = Math.min(
      WS_RECONNECT_BASE_DELAY * Math.pow(2, reconnectAttemptRef.current),
      WS_RECONNECT_MAX_DELAY,
    );
    reconnectAttemptRef.current++;

    reconnectTimeoutRef.current = setTimeout(() => {
      connect();
    }, delay);
  }, [connect]);

  const disconnect = useCallback(() => {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = undefined;
    }

    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    isConnectingRef.current = false;
    clearAll();
  }, [clearAll]);

  // Connection lifecycle
  useEffect(() => {
    if (isAuthenticated && accessToken) {
      connect();
    } else {
      disconnect();
    }

    return () => {
      disconnect();
    };
  }, [isAuthenticated, accessToken, connect, disconnect]);

  // No-op for now as we use global subscription
  const subscribe = useCallback((_streamerId: string) => { }, []);
  const unsubscribe = useCallback((_streamerId: string) => { }, []);

  return (
    <WebSocketContext.Provider
      value={{ isConnected: !!wsRef.current, subscribe, unsubscribe }}
    >
      {children}
    </WebSocketContext.Provider>
  );
}
