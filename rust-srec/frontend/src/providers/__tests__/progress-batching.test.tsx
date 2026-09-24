import { act, cleanup, render } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { create, toBinary } from '@bufbuild/protobuf';

import {
  EventType,
  WsMessageSchema,
  type WsMessage,
} from '@/api/proto/gen/download_progress_pb.js';
import type { MessageInitShape } from '@bufbuild/protobuf';
import { useDownloadStore } from '@/store/downloads';
import { useUploadStore } from '@/store/uploads';
import { PROGRESS_FLUSH_MS, WebSocketProvider } from '../WebSocketProvider';

const routeContext = vi.hoisted(() => ({
  user: null as unknown,
}));

vi.mock('@tanstack/react-router', () => ({
  useRouteContext: () => routeContext,
}));

class MockWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: MockWebSocket[] = [];

  readonly url: string;
  readyState = MockWebSocket.CONNECTING;
  binaryType: BinaryType = 'blob';
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  send = vi.fn();

  constructor(url: string | URL) {
    this.url = url.toString();
    MockWebSocket.instances.push(this);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
  }

  emitOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event('open'));
  }

  emit(message: MessageInitShape<typeof WsMessageSchema>) {
    const bytes = toBinary(
      WsMessageSchema,
      create(WsMessageSchema, message) as WsMessage,
    );
    const data = bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength,
    );
    this.onmessage?.(new MessageEvent('message', { data }));
  }
}

const meta = {
  downloadId: 'download-1',
  streamerId: 'streamer-1',
  sessionId: 'session-1',
  engineType: 'mesio',
  startedAtMs: 0n,
  updatedAtMs: 0n,
  cdnHost: '',
  downloadUrl: '',
};

function metrics(bytesDownloaded: bigint) {
  return {
    downloadId: 'download-1',
    status: 'Downloading',
    bytesDownloaded,
    durationSecs: 1,
    speedBytesPerSec: 1n,
    segmentsCompleted: 0,
    mediaDurationSecs: 1,
    playbackRatio: 1,
  };
}

function bytesOf(downloadId: string) {
  return useDownloadStore.getState().viewsById.get(downloadId)?.bytesDownloaded;
}

function renderProvider() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <WebSocketProvider>
        <div>child</div>
      </WebSocketProvider>
    </QueryClientProvider>,
  );
  const socket = MockWebSocket.instances[0];
  act(() => socket.emitOpen());
  return socket;
}

describe('WebSocketProvider progress batching', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances = [];
    routeContext.user = {
      username: 'user',
      token: {
        access_token: 'token-a',
        expires_in: Date.now() + 60 * 60_000,
        refresh_expires_in: Date.now() + 24 * 60 * 60_000,
      },
      roles: [],
      mustChangePassword: false,
    };
    vi.stubGlobal('WebSocket', MockWebSocket);
  });

  afterEach(() => {
    cleanup();
    useDownloadStore.getState().clearAll();
    useUploadStore.getState().clearAll();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('applies a window of progress ticks as one store update', () => {
    const socket = renderProvider();
    act(() => {
      socket.emit({
        eventType: EventType.DOWNLOAD_META,
        payload: { case: 'downloadMeta', value: meta },
      });
    });
    const versionBefore = useDownloadStore.getState().version;

    act(() => {
      for (const bytes of [1n, 2n, 3n]) {
        socket.emit({
          eventType: EventType.DOWNLOAD_METRICS,
          payload: { case: 'downloadMetrics', value: metrics(bytes) },
        });
      }
      socket.emit({
        eventType: EventType.UPLOAD_PROGRESS,
        payload: {
          case: 'uploadProgress',
          value: { jobId: 'job-1', streamerId: 'streamer-1', percent: 40 },
        },
      });
    });
    expect(bytesOf('download-1')).toBe(0n);
    expect(useUploadStore.getState().uploadsByJobId.has('job-1')).toBe(false);

    act(() => {
      vi.advanceTimersByTime(PROGRESS_FLUSH_MS);
    });

    expect(bytesOf('download-1')).toBe(3n);
    expect(useDownloadStore.getState().version).toBe(versionBefore + 1);
    expect(useUploadStore.getState().uploadsByJobId.get('job-1')?.percent).toBe(
      40,
    );
  });

  it('applies held ticks before any other event, keeping server order', () => {
    const socket = renderProvider();

    act(() => {
      socket.emit({
        eventType: EventType.DOWNLOAD_METRICS,
        payload: { case: 'downloadMetrics', value: metrics(5n) },
      });
      // A newer snapshot must not be overwritten by the older tick.
      socket.emit({
        eventType: EventType.SNAPSHOT,
        payload: {
          case: 'snapshot',
          value: { downloads: [{ meta, metrics: metrics(10n) }] },
        },
      });
    });
    expect(bytesOf('download-1')).toBe(10n);

    act(() => {
      vi.advanceTimersByTime(PROGRESS_FLUSH_MS);
    });
    expect(bytesOf('download-1')).toBe(10n);
  });

  it('does not bring back a download that finished after its last tick', () => {
    const socket = renderProvider();
    act(() => {
      socket.emit({
        eventType: EventType.DOWNLOAD_META,
        payload: { case: 'downloadMeta', value: meta },
      });
      socket.emit({
        eventType: EventType.DOWNLOAD_METRICS,
        payload: { case: 'downloadMetrics', value: metrics(5n) },
      });
      socket.emit({
        eventType: EventType.DOWNLOAD_COMPLETED,
        payload: {
          case: 'downloadCompleted',
          value: { downloadId: 'download-1', streamerId: 'streamer-1' },
        },
      });
      vi.advanceTimersByTime(PROGRESS_FLUSH_MS);
    });

    expect(useDownloadStore.getState().viewsById.has('download-1')).toBe(false);
  });
});
