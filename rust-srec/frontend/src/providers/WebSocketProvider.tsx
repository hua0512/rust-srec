import { useEffect, useRef, useCallback, ReactNode } from 'react';
import { useRouteContext } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { sessionQueryOptions } from '@/api/session';
import { useDownloadStore } from '@/store/downloads';
import {
  decodeWsMessage,
  encodeClientMessage,
  EventType,
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
  const { user: routeUser } = useRouteContext({ from: '__root__' }) as {
    user?: any;
  };
  const { data: sessionData } = useQuery({
    ...sessionQueryOptions,
    enabled: typeof window !== 'undefined',
    initialData: routeUser ?? null,
    refetchInterval: 60_000,
    refetchIntervalInBackground: true,
  });
  const accessToken = sessionData?.token?.access_token;
  const isAuthenticated = !!accessToken;

  // Download store actions
  const setSnapshot = useDownloadStore((state) => state.setSnapshot);
  const upsertMeta = useDownloadStore((state) => state.upsertMeta);
  const upsertMetrics = useDownloadStore((state) => state.upsertMetrics);
  const removeDownload = useDownloadStore((state) => state.removeDownload);
  const setConnectionStatus = useDownloadStore(
    (state) => state.setConnectionStatus,
  );
  const connectionStatus = useDownloadStore((state) => state.connectionStatus);
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

          case EventType.EVENT_TYPE_DOWNLOAD_META:
            if ('downloadMeta' in message.payload) {
              const meta = message.payload.downloadMeta;
              upsertMeta(meta);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_METRICS:
            if ('downloadMetrics' in message.payload) {
              const m = message.payload.downloadMetrics;
              upsertMetrics(m);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_COMPLETED:
            if ('downloadCompleted' in message.payload) {
              // Terminal event - remove from active list.
              removeDownload(message.payload.downloadCompleted.downloadId);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_FAILED:
            if ('downloadFailed' in message.payload) {
              // Terminal event - remove from active list.
              removeDownload(message.payload.downloadFailed.downloadId);
            }
            break;

          case EventType.EVENT_TYPE_DOWNLOAD_CANCELLED:
            if ('downloadCancelled' in message.payload) {
              // Terminal event - remove from active list.
              removeDownload(message.payload.downloadCancelled.downloadId);
            }
            break;

          case EventType.EVENT_TYPE_SEGMENT_COMPLETED:
          case EventType.EVENT_TYPE_DOWNLOAD_REJECTED:
          case EventType.EVENT_TYPE_ERROR:
            // Not currently surfaced in the UI; decoding still works.
            break;
        }
      } catch (error) {
        console.error('Failed to decode WebSocket message:', error);
      }
    },
    [setSnapshot, upsertMeta, upsertMetrics, removeDownload],
  );

  const connect = useCallback(() => {
    if (!accessToken || !isAuthenticated) return;
    if (typeof window === 'undefined') return;
    if (isConnectingRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;
    if (wsRef.current?.readyState === WebSocket.CONNECTING) return;

    isConnectingRef.current = true;
    setConnectionStatus('connecting');

    const wsUrl = buildWebSocketUrl(accessToken);
    if (import.meta.env.DEV) {
      console.debug('[WS] Connecting to', wsUrl);
    }
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

    ws.onclose = (event) => {
      if (import.meta.env.DEV) {
        console.debug('[WS] Close', {
          code: event.code,
          reason: event.reason,
          wasClean: event.wasClean,
        });
      }
      console.debug('[WS] Disconnected');
      isConnectingRef.current = false;
      setConnectionStatus('disconnected');
      wsRef.current = null;

      if (sessionData?.token?.access_token) {
        scheduleReconnect();
      }
    };

    ws.onerror = (event) => {
      if (import.meta.env.DEV) {
        console.error('[WS] Connection error', event);
      } else {
        console.error('[WS] Connection error');
      }
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

  const subscribe = useCallback((streamerId: string) => {
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const msg = encodeClientMessage({
      action: { subscribe: { streamerId } },
    });
    ws.send(msg);
  }, []);

  const unsubscribe = useCallback((_streamerId: string) => {
    // Protocol unsubscribe is global (clears filter).
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const msg = encodeClientMessage({
      action: { unsubscribe: {} },
    });
    ws.send(msg);
  }, []);

  return (
    <WebSocketContext.Provider
      value={{
        isConnected: connectionStatus === 'connected',
        subscribe,
        unsubscribe,
      }}
    >
      {children}
    </WebSocketContext.Provider>
  );
}
