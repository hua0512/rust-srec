/**
 * Owns the lifecycle of a token-authenticated WebSocket: opening, reconnecting
 * with backoff after an unexpected drop, and closing for good when the session
 * ends or the caller unmounts. Callers supply what is specific to their stream
 * (endpoint, handshake message, payload handling) and keep no socket state of
 * their own.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { buildWebSocketUrl } from '@/lib/url';

// Reconnection constants
const WS_RECONNECT_BASE_DELAY = 1000;
const WS_RECONNECT_MAX_DELAY = 30000;

export type WebSocketStatus =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'error';

export interface UseAuthedWebSocketOptions {
  /** Current access token; the socket is opened only while one is present. */
  accessToken: string | undefined;
  /** Endpoint below the API base; defaults to the download progress stream. */
  path?: string;
  /** Prefix for this stream's dev-only console diagnostics. */
  debugLabel?: string;
  onMessage: (event: MessageEvent) => void;
  /** Runs on every successful handshake, e.g. to send a subscription. */
  onOpen?: (socket: WebSocket) => void;
  onStatusChange?: (status: WebSocketStatus) => void;
  /**
   * Runs on an intentional close — the session ended or the caller unmounted —
   * and never on a reconnect, so callers can tear down state that only makes
   * sense while a session is live.
   */
  onDisconnect?: () => void;
}

export interface AuthedWebSocket {
  status: WebSocketStatus;
  /** Sends on the open socket; a no-op while there is none. */
  send: (data: string | Blob | BufferSource) => void;
  /** Closes for good: no reconnect follows until the session ends and a new one begins. */
  disconnect: () => void;
}

export function useAuthedWebSocket(
  options: UseAuthedWebSocketOptions,
): AuthedWebSocket {
  const { accessToken, path, debugLabel = 'WS' } = options;
  const [status, setStatus] = useState<WebSocketStatus>('disconnected');

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptRef = useRef<number>(0);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const isConnectingRef = useRef<boolean>(false);
  const intentionalCloseRef = useRef<boolean>(false);
  // Reconnects go through a ref so scheduleReconnect stays independent of
  // connect, which in turn lets connect stay stable across renders.
  const connectRef = useRef<() => void>(() => {});

  // Caller-supplied behavior is read from refs at event time, so a caller may
  // pass inline callbacks without any of them reopening the socket.
  const pathRef = useRef(path);
  const debugLabelRef = useRef(debugLabel);
  const onMessageRef = useRef(options.onMessage);
  const onOpenRef = useRef(options.onOpen);
  const onStatusChangeRef = useRef(options.onStatusChange);
  const onDisconnectRef = useRef(options.onDisconnect);
  useEffect(() => {
    pathRef.current = path;
    debugLabelRef.current = debugLabel;
    onMessageRef.current = options.onMessage;
    onOpenRef.current = options.onOpen;
    onStatusChangeRef.current = options.onStatusChange;
    onDisconnectRef.current = options.onDisconnect;
  });

  // Read by connect() instead of captured, so renewing the access token does
  // not change the connect callback and therefore does not restart the socket.
  const accessTokenRef = useRef<string | undefined>(accessToken);
  useEffect(() => {
    accessTokenRef.current = accessToken;
    // A reconnect already waiting out its backoff would hand the handshake
    // whichever token it finds when the timer fires. Once a renewed one is in
    // hand there is nothing left to wait for, so retry with it immediately
    // rather than let the pending attempt run on a token that has since
    // expired.
    if (!accessToken) return;
    if (!reconnectTimeoutRef.current) return;
    clearTimeout(reconnectTimeoutRef.current);
    reconnectTimeoutRef.current = undefined;
    connectRef.current();
  }, [accessToken]);

  const applyStatus = useCallback((next: WebSocketStatus) => {
    setStatus(next);
    onStatusChangeRef.current?.(next);
  }, []);

  const scheduleReconnect = useCallback(() => {
    const delay = Math.min(
      WS_RECONNECT_BASE_DELAY * Math.pow(2, reconnectAttemptRef.current),
      WS_RECONNECT_MAX_DELAY,
    );
    reconnectAttemptRef.current++;

    reconnectTimeoutRef.current = setTimeout(() => {
      connectRef.current();
    }, delay);
  }, []);

  // Deliberately free of the access token: an open socket keeps the
  // credentials it was opened with, so a renewed token is only needed by the
  // next connect attempt and is read from the ref at that point.
  const connect = useCallback(() => {
    const token = accessTokenRef.current;
    if (!token) return;
    if (typeof window === 'undefined') return;
    if (isConnectingRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;
    if (wsRef.current?.readyState === WebSocket.CONNECTING) return;

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = undefined;
    }
    intentionalCloseRef.current = false;
    isConnectingRef.current = true;
    applyStatus('connecting');

    const label = debugLabelRef.current;
    const wsUrl = buildWebSocketUrl(token, pathRef.current);
    if (import.meta.env.DEV) {
      console.debug(`[${label}] Connecting to`, wsUrl);
    }
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      if (wsRef.current !== ws) {
        ws.close();
        return;
      }
      if (import.meta.env.DEV) {
        console.debug(`[${label}] Connected`);
      }
      isConnectingRef.current = false;
      applyStatus('connected');
      reconnectAttemptRef.current = 0;
      onOpenRef.current?.(ws);
    };

    ws.onmessage = (event) => {
      if (wsRef.current === ws) onMessageRef.current(event);
    };

    ws.onclose = (event) => {
      if (wsRef.current !== ws) return;

      if (import.meta.env.DEV) {
        console.debug(`[${label}] Disconnected`, {
          code: event.code,
          reason: event.reason,
          wasClean: event.wasClean,
        });
      }
      isConnectingRef.current = false;
      applyStatus('disconnected');
      wsRef.current = null;

      if (!intentionalCloseRef.current && accessTokenRef.current) {
        scheduleReconnect();
      }
    };

    ws.onerror = (event) => {
      if (wsRef.current !== ws) return;

      if (import.meta.env.DEV) {
        console.error(`[${label}] Connection error`, event);
      } else {
        console.error(`[${label}] Connection error`);
      }
      isConnectingRef.current = false;
      applyStatus('error');
    };

    wsRef.current = ws;
  }, [applyStatus, scheduleReconnect]);

  useEffect(() => {
    connectRef.current = connect;
  }, [connect]);

  const disconnect = useCallback(() => {
    // The lifecycle effect's cleanup and its next run both land here when a
    // session ends; the second call finds nothing open and nothing pending.
    if (
      intentionalCloseRef.current &&
      !wsRef.current &&
      !reconnectTimeoutRef.current
    ) {
      return;
    }
    intentionalCloseRef.current = true;

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = undefined;
    }

    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    isConnectingRef.current = false;
    applyStatus('disconnected');
    onDisconnectRef.current?.();
  }, [applyStatus]);

  // Connection lifecycle. Keyed on whether there is a session at all, not on
  // the token itself: renewing the access token leaves the open socket alone.
  const isAuthenticated = !!accessToken;
  useEffect(() => {
    if (isAuthenticated) {
      connect();
    } else {
      disconnect();
    }

    return () => {
      disconnect();
    };
  }, [isAuthenticated, connect, disconnect]);

  const send = useCallback((data: string | Blob | BufferSource) => {
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(data);
  }, []);

  return { status, send, disconnect };
}
