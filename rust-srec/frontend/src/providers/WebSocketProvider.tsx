import { useEffect, useRef, useCallback, ReactNode } from 'react';
import { useRouteContext } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { fromBinary, toBinary, create } from '@bufbuild/protobuf';
import { sessionQueryOptions } from '@/api/session';
import { useDownloadStore } from '@/store/downloads';
import {
  WsMessageSchema,
  ClientMessageSchema,
  SubscribeRequestSchema,
  UnsubscribeRequestSchema,
  EventType,
} from '@/api/proto/gen/download_progress_pb.js';
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
        const message = fromBinary(WsMessageSchema, data);
        // console.debug('[WS] Received message:', message.eventType);

        switch (message.eventType) {
          case EventType.SNAPSHOT:
            if (message.payload.case === 'snapshot') {
              setSnapshot(message.payload.value.downloads);
            }
            break;

          case EventType.DOWNLOAD_META:
            if (message.payload.case === 'downloadMeta') {
              upsertMeta(message.payload.value);
            }
            break;

          case EventType.DOWNLOAD_METRICS:
            if (message.payload.case === 'downloadMetrics') {
              upsertMetrics(message.payload.value);
            }
            break;

          case EventType.DOWNLOAD_COMPLETED:
            if (message.payload.case === 'downloadCompleted') {
              // Terminal event - remove from active list.
              removeDownload(message.payload.value.downloadId);
            }
            break;

          case EventType.DOWNLOAD_FAILED:
            if (message.payload.case === 'downloadFailed') {
              // Terminal event - remove from active list.
              removeDownload(message.payload.value.downloadId);
            }
            break;

          case EventType.DOWNLOAD_CANCELLED:
            if (message.payload.case === 'downloadCancelled') {
              // Terminal event - remove from active list.
              removeDownload(message.payload.value.downloadId);
            }
            break;

          case EventType.SEGMENT_COMPLETED:
          case EventType.DOWNLOAD_REJECTED:
          case EventType.ERROR:
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
      const unsubscribeReq = create(UnsubscribeRequestSchema, {});
      const clientMessage = create(ClientMessageSchema, {
        action: { case: 'unsubscribe', value: unsubscribeReq },
      });
      ws.send(toBinary(ClientMessageSchema, clientMessage));
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
    const subscribeReq = create(SubscribeRequestSchema, { streamerId });
    const clientMessage = create(ClientMessageSchema, {
      action: { case: 'subscribe', value: subscribeReq },
    });
    ws.send(toBinary(ClientMessageSchema, clientMessage));
  }, []);

  const unsubscribe = useCallback((_streamerId: string) => {
    // Protocol unsubscribe is global (clears filter).
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const unsubscribeReq = create(UnsubscribeRequestSchema, {});
    const clientMessage = create(ClientMessageSchema, {
      action: { case: 'unsubscribe', value: unsubscribeReq },
    });
    ws.send(toBinary(ClientMessageSchema, clientMessage));
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
