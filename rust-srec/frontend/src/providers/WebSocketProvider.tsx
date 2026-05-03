import { useEffect, useRef, useCallback, ReactNode } from 'react';
import { useRouteContext } from '@tanstack/react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
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
import {
  StreamerCheckHistoryEntrySchema,
  type StreamerCheckHistoryEntry,
} from '@/server/functions/streamers';
import type { QueryClient } from '@tanstack/react-query';

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
  const { user: routeUser } = useRouteContext({ from: '/_authed' }) as {
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

  // Query cache used for the check-history strip's React Query state.
  // The component subscribes to ['streamer', streamerId, 'check-history', N];
  // we mutate every matching entry on each WS event so multiple open
  // strips for the same streamer (different slot counts) all stay live.
  const queryClient = useQueryClient();

  const handleMessage = useCallback(
    (event: MessageEvent) => {
      try {
        const data = new Uint8Array(event.data as ArrayBuffer);
        const message = fromBinary(WsMessageSchema, data);
        // console.debug('[WS] Received message:', message.eventType);

        switch (message.eventType) {
          case EventType.SNAPSHOT:
            if (message.payload.case === 'snapshot') {
              type SnapshotItem = Parameters<typeof setSnapshot>[0][number];

              const downloads: Parameters<typeof setSnapshot>[0] =
                message.payload.value.downloads
                  .map((d): SnapshotItem | null => {
                    const downloadId =
                      d.meta?.downloadId || d.metrics?.downloadId;
                    if (!downloadId) return null;
                    return {
                      meta: d.meta ?? {
                        downloadId,
                        streamerId: '',
                        sessionId: '',
                        engineType: '',
                        startedAtMs: 0n,
                        updatedAtMs: 0n,
                        cdnHost: '',
                        downloadUrl: '',
                      },
                      metrics: d.metrics ?? {
                        downloadId,
                        status: '',
                        bytesDownloaded: 0n,
                        durationSecs: 0,
                        speedBytesPerSec: 0n,
                        segmentsCompleted: 0,
                        mediaDurationSecs: 0,
                        playbackRatio: 0,
                      },
                    };
                  })
                  .filter((d): d is SnapshotItem => d !== null);

              setSnapshot(downloads);
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

          case EventType.DOWNLOAD_REJECTED:
          case EventType.ERROR:
            // Not currently surfaced in the UI; decoding still works.
            break;

          case EventType.STREAMER_CHECK_RECORDED:
            if (message.payload.case === 'streamerCheckRecorded') {
              const wire = message.payload.value;
              // Translate the proto shape (empty strings for absent values)
              // back into the REST shape (null for absent) so the cache
              // entries stay homogeneous regardless of source. Defensive
              // parse on the JSON fields for the same reason the REST
              // handler does it: malformed JSON degrades to null rather
              // than dropping the bar.
              let parsedSelected: unknown = null;
              if (wire.streamSelectedJson) {
                try {
                  parsedSelected = JSON.parse(wire.streamSelectedJson);
                } catch {
                  parsedSelected = null;
                }
              }
              let parsedExtracted: unknown = null;
              if (wire.streamsExtractedJson) {
                try {
                  parsedExtracted = JSON.parse(wire.streamsExtractedJson);
                } catch {
                  parsedExtracted = null;
                }
              }
              const candidate = {
                checked_at: new Date(Number(wire.checkedAtMs)).toISOString(),
                duration_ms: wire.durationMs,
                outcome: wire.outcome,
                fatal_kind: wire.fatalKind || null,
                filter_reason: wire.filterReason || null,
                error_message: wire.errorMessage || null,
                streams_extracted: wire.streamsExtracted,
                stream_selected: parsedSelected,
                streams_extracted_detail: parsedExtracted,
                title: wire.title || null,
                category: wire.category || null,
                viewer_count:
                  wire.viewerCount === 0n ? null : Number(wire.viewerCount),
              };
              const parsed =
                StreamerCheckHistoryEntrySchema.safeParse(candidate);
              if (!parsed.success) {
                console.warn(
                  '[WS] Discarded malformed StreamerCheckRecorded',
                  parsed.error.message,
                );
                break;
              }
              const entry: StreamerCheckHistoryEntry = parsed.data;
              appendCheckHistoryEntry(queryClient, wire.streamerId, entry);
            }
            break;
        }
      } catch (error) {
        console.error('Failed to decode WebSocket message:', error);
      }
    },
    [setSnapshot, upsertMeta, upsertMetrics, removeDownload, queryClient],
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

/** Cap kept in sync with the backend's `KEEP_PER_STREAMER` repository
 *  constant. Cache entries longer than this are useless to the strip and
 *  just bloat memory; we trim the head on every WS-driven append. */
const CHECK_HISTORY_CACHE_CAP = 200;

/** Append a freshly-broadcast row to every cached `check-history` query for
 *  this streamer. Multiple slot counts (e.g. a tab with the strip and a
 *  popout with a longer view) end up as separate cache entries; mutating
 *  every match means all open views stay live without each component
 *  having to manage its own subscription. */
function appendCheckHistoryEntry(
  queryClient: QueryClient,
  streamerId: string,
  entry: StreamerCheckHistoryEntry,
) {
  type CacheShape = { items: StreamerCheckHistoryEntry[] };
  queryClient.setQueriesData<CacheShape>(
    {
      // Match every limit variant for this streamer.
      // queryKey: ['streamer', streamerId, 'check-history', limit]
      predicate: (query) => {
        const key = query.queryKey;
        return (
          Array.isArray(key) &&
          key[0] === 'streamer' &&
          key[1] === streamerId &&
          key[2] === 'check-history'
        );
      },
    },
    (prev) => {
      // No cache yet (the strip hasn't mounted) — let the initial fetch
      // populate it; the same row will be in the REST response anyway.
      if (!prev) return prev;
      // De-dupe: a refetch may race with a WS event and land the same row
      // twice. Key on (checked_at, outcome) — the writer guarantees one
      // row per poll, so the timestamp is unique.
      const dupeKey = `${entry.checked_at}|${entry.outcome}`;
      const seen = prev.items.some(
        (i) => `${i.checked_at}|${i.outcome}` === dupeKey,
      );
      if (seen) return prev;
      const next = [...prev.items, entry];
      // Server returns oldest-first; trim the oldest if we're over cap.
      if (next.length > CHECK_HISTORY_CACHE_CAP) {
        next.splice(0, next.length - CHECK_HISTORY_CACHE_CAP);
      }
      return { items: next };
    },
  );
}
