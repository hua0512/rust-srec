import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

import { LogViewer } from '@/components/logging/log-viewer';
import { useDownloadStore } from '@/store/downloads';
import { useUploadStore } from '@/store/uploads';
import { WebSocketProvider } from '../WebSocketProvider';

function sessionWithToken(accessToken: string) {
  return {
    username: 'user',
    token: {
      access_token: accessToken,
      expires_in: Date.now() + 60 * 60_000,
      refresh_expires_in: Date.now() + 24 * 60 * 60_000,
    },
    roles: [],
    mustChangePassword: false,
  };
}

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

  emitClose() {
    this.onclose?.(new CloseEvent('close'));
  }
}

function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity },
    },
  });
}

async function rotateToken(queryClient: QueryClient) {
  await act(async () => {
    queryClient.setQueryData(['session'], sessionWithToken('token-b'));
    await vi.advanceTimersByTimeAsync(0);
  });
}

/** A download and an upload, as the server's snapshot would deliver them. */
function seedLiveTransfers() {
  useDownloadStore.getState().setSnapshot(
    [
      {
        meta: {
          downloadId: 'download-1',
          streamerId: 'streamer-1',
          sessionId: 'session-1',
          engineType: 'flv',
          startedAtMs: 0n,
          updatedAtMs: 0n,
          cdnHost: '',
          downloadUrl: '',
        },
        metrics: {
          downloadId: 'download-1',
          status: 'DOWNLOADING',
          bytesDownloaded: 1n,
          durationSecs: 1,
          speedBytesPerSec: 1n,
          segmentsCompleted: 0,
          mediaDurationSecs: 1,
          playbackRatio: 1,
        },
      },
    ],
    [],
  );
  useUploadStore.getState().setSnapshot(
    [
      {
        jobId: 'job-1',
        streamerId: 'streamer-1',
        sessionId: 'session-1',
        uploader: 'rclone',
        filesTotal: 2,
        startedAtMs: 0n,
      },
    ],
    [],
  );
}

describe('WebSocket lifecycle ownership', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances = [];
    routeContext.user = sessionWithToken('token-a');
    vi.stubGlobal('WebSocket', MockWebSocket);
  });

  afterEach(() => {
    cleanup();
    useDownloadStore.getState().clearAll();
    useUploadStore.getState().clearAll();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('keeps the provider socket open across a token rotation', async () => {
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <WebSocketProvider>
          <div>child</div>
        </WebSocketProvider>
      </QueryClientProvider>,
    );

    const socket = MockWebSocket.instances[0];
    act(() => socket.emitOpen());
    seedLiveTransfers();

    await rotateToken(queryClient);
    act(() => {
      vi.advanceTimersByTime(WS_RECONNECT_WINDOW_MS);
    });

    // The handshake already authenticated this socket, so a renewed token is
    // no reason to replace it.
    expect(MockWebSocket.instances).toHaveLength(1);
    expect(socket.readyState).toBe(MockWebSocket.OPEN);
  });

  it('keeps live transfers on screen across a token rotation', async () => {
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <WebSocketProvider>
          <div>child</div>
        </WebSocketProvider>
      </QueryClientProvider>,
    );

    act(() => MockWebSocket.instances[0].emitOpen());
    seedLiveTransfers();

    await rotateToken(queryClient);

    expect(useDownloadStore.getState().viewsById.has('download-1')).toBe(true);
    expect(useUploadStore.getState().uploadsByJobId.has('job-1')).toBe(true);
  });

  it('reopens the provider socket with the renewed token after it drops', async () => {
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <WebSocketProvider>
          <div>child</div>
        </WebSocketProvider>
      </QueryClientProvider>,
    );

    const firstSocket = MockWebSocket.instances[0];
    act(() => firstSocket.emitOpen());
    await rotateToken(queryClient);

    act(() => {
      firstSocket.emitClose();
      vi.advanceTimersByTime(WS_RECONNECT_WINDOW_MS);
    });

    expect(MockWebSocket.instances).toHaveLength(2);
    expect(MockWebSocket.instances[1].url).toContain('token-b');

    // The socket that already handed over is not allowed to schedule another
    // reconnect on top of the live one.
    act(() => {
      firstSocket.emitClose();
      vi.advanceTimersByTime(WS_RECONNECT_WINDOW_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it('ignores a stale log socket close after token rotation', async () => {
    const queryClient = createQueryClient();
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    render(
      <QueryClientProvider client={queryClient}>
        <I18nProvider i18n={i18n}>
          <LogViewer />
        </I18nProvider>
      </QueryClientProvider>,
    );

    const staleSocket = MockWebSocket.instances[0];
    await rotateToken(queryClient);
    expect(MockWebSocket.instances).toHaveLength(2);

    act(() => {
      staleSocket.emitClose();
      vi.advanceTimersByTime(WS_RECONNECT_WINDOW_MS);
    });

    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it('keeps the log socket open when pausing the viewer', async () => {
    const queryClient = createQueryClient();
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    render(
      <QueryClientProvider client={queryClient}>
        <I18nProvider i18n={i18n}>
          <LogViewer />
        </I18nProvider>
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    expect(MockWebSocket.instances).toHaveLength(1);
  });
});

const WS_RECONNECT_WINDOW_MS = 30_000;
