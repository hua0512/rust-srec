import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react';
import { type QueryClient, QueryClientProvider } from '@tanstack/react-query';

import { queryKeys } from '@/api/query-keys';
import { LogViewer } from '@/components/logging/log-viewer';
import { useDownloadStore } from '@/store/downloads';
import { useUploadStore } from '@/store/uploads';
import { WebSocketProvider } from '../WebSocketProvider';
import { newQueryClient, renderWithProviders } from '@/test/render';
import { MockWebSocket } from '@/test/mock-websocket';
import { testSession } from '@/test/session';

const routeContext = vi.hoisted(() => ({
  user: null as unknown,
}));

vi.mock('@tanstack/react-router', () => ({
  useRouteContext: () => routeContext,
}));

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions/auth', () => ({
  checkAuthFn: checkAuthFnMock,
}));

async function rotateToken(queryClient: QueryClient) {
  await act(async () => {
    queryClient.setQueryData(
      queryKeys.session,
      testSession({ accessToken: 'token-b' }),
    );
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
        streamerName: 'Streamer One',
        streamerAvatar: '',
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
    routeContext.user = testSession({ accessToken: 'token-a' });
    checkAuthFnMock.mockReset();
    checkAuthFnMock.mockRejectedValue(new Error('offline'));
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
    const queryClient = newQueryClient();
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
    const queryClient = newQueryClient();
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
    const queryClient = newQueryClient();
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

  it('renews an expiring session when the provider socket drops', async () => {
    // Inside the renewal window, as when the backend closes the socket at
    // token expiry.
    routeContext.user = testSession({
      accessToken: 'token-a',
      expiresInMs: 10_000,
    });
    const queryClient = newQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <WebSocketProvider>
          <div>child</div>
        </WebSocketProvider>
      </QueryClientProvider>,
    );

    const firstSocket = MockWebSocket.instances[0];
    // Let the check on mount fail first, so only the drop can renew.
    await act(async () => {
      firstSocket.emitOpen();
      await vi.advanceTimersByTimeAsync(0);
    });
    checkAuthFnMock.mockResolvedValue(testSession({ accessToken: 'token-b' }));

    await act(async () => {
      firstSocket.emitClose();
      await vi.advanceTimersByTimeAsync(0);
    });

    // Reopened with the renewed token without waiting out the backoff.
    expect(MockWebSocket.instances).toHaveLength(2);
    expect(MockWebSocket.instances[1].url).toContain('token-b');
  });

  it('keeps the log socket open across a token rotation and reopens it once after a drop', async () => {
    const queryClient = newQueryClient();
    renderWithProviders(<LogViewer />, { client: queryClient });

    const firstSocket = MockWebSocket.instances[0];
    act(() => firstSocket.emitOpen());
    await rotateToken(queryClient);

    // Same rule as the provider socket: the handshake already authenticated it.
    expect(MockWebSocket.instances).toHaveLength(1);

    act(() => {
      firstSocket.emitClose();
      vi.advanceTimersByTime(WS_RECONNECT_WINDOW_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(2);
    expect(MockWebSocket.instances[1].url).toContain('token-b');

    act(() => {
      firstSocket.emitClose();
      vi.advanceTimersByTime(WS_RECONNECT_WINDOW_MS);
    });

    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it('keeps the log socket open when pausing the viewer', async () => {
    const queryClient = newQueryClient();
    renderWithProviders(<LogViewer />, { client: queryClient });

    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    expect(MockWebSocket.instances).toHaveLength(1);
  });
});

const WS_RECONNECT_WINDOW_MS = 30_000;
