import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import { create, toBinary } from '@bufbuild/protobuf';

import {
  EventType,
  LogEventSchema,
  LogLevel,
  WsMessageSchema,
} from '@/api/proto/gen/log_event_pb.js';
import { LogViewer } from '../log-viewer';
import { renderWithProviders } from '@/test/render';
import { MockWebSocket } from '@/test/mock-websocket';
import { testSession } from '@/test/session';

const FLUSH_WINDOW_MS = 50;
const MAX_LOG_ENTRIES = 500;

const routeContext = vi.hoisted(() => ({
  user: null as unknown,
}));

vi.mock('@tanstack/react-router', () => ({
  useRouteContext: () => routeContext,
}));

function emitLog(socket: MockWebSocket, index: number) {
  const message = create(WsMessageSchema, {
    eventType: EventType.LOG,
    payload: {
      case: 'log',
      value: create(LogEventSchema, {
        timestampMs: BigInt(1_700_000_000_000 + index),
        level: LogLevel.INFO,
        target: 'test',
        message: `log-${index}`,
      }),
    },
  });
  socket.emitBinary(toBinary(WsMessageSchema, message));
}

function renderViewer() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  const utils = renderWithProviders(<LogViewer />, { client: queryClient });
  const socket = MockWebSocket.instances[0];
  act(() => socket.emitOpen());
  return { ...utils, socket };
}

/** Rows live directly inside the scrolling log pane; the empty state does not. */
function renderedRowCount(container: HTMLElement): number {
  const pane = container.querySelector('.overflow-y-auto');
  return pane?.querySelectorAll(':scope > .items-start').length ?? 0;
}

function emitLogs(socket: MockWebSocket, count: number, offset = 0) {
  act(() => {
    for (let i = 0; i < count; i++) emitLog(socket, offset + i);
  });
}

function flush() {
  act(() => {
    vi.advanceTimersByTime(FLUSH_WINDOW_MS);
  });
}

describe('LogViewer', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances = [];
    routeContext.user = testSession({ accessToken: 'token-a' });
    vi.stubGlobal('WebSocket', MockWebSocket);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('shows a burst of frames only once the flush window elapses', () => {
    const { container, socket } = renderViewer();

    emitLogs(socket, 40);

    // Nothing is handed to React while the window is open, no matter how many
    // frames arrive in it.
    expect(renderedRowCount(container)).toBe(0);
    expect(screen.getByText('No logs to display')).toBeInTheDocument();

    flush();

    expect(renderedRowCount(container)).toBe(40);
    expect(screen.getByText('log-0')).toBeInTheDocument();
    expect(screen.getByText('log-39')).toBeInTheDocument();
  });

  // Rendering and querying hundreds of rows in jsdom takes about half of the
  // default 5 s budget in a normal parallel run, so extra load times it out.
  it(
    'keeps only the newest entries once past the cap',
    { timeout: 15_000 },
    () => {
      const { container, socket } = renderViewer();

      emitLogs(socket, MAX_LOG_ENTRIES + 20);
      flush();

      expect(renderedRowCount(container)).toBe(MAX_LOG_ENTRIES);
      expect(screen.queryByText('log-0')).not.toBeInTheDocument();
      expect(screen.getByText('log-519')).toBeInTheDocument();
    },
  );

  it('holds frames back while paused and merges them on resume', () => {
    const { container, socket } = renderViewer();

    emitLogs(socket, 3);
    flush();
    expect(renderedRowCount(container)).toBe(3);

    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));

    emitLogs(socket, 2, 3);
    flush();

    expect(renderedRowCount(container)).toBe(3);
    expect(screen.queryByText('log-3')).not.toBeInTheDocument();
    expect(screen.getByText('(+2 paused)')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Resume' }));

    expect(renderedRowCount(container)).toBe(5);
    expect(screen.getByText('log-4')).toBeInTheDocument();
  });

  it('drops buffered frames when the log is cleared', () => {
    const { container, socket } = renderViewer();

    emitLogs(socket, 5);
    flush();
    emitLogs(socket, 5, 5);

    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    flush();

    expect(renderedRowCount(container)).toBe(0);
    expect(screen.getByText('No logs to display')).toBeInTheDocument();
  });
});
