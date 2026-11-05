import { act, cleanup, fireEvent, screen } from '@testing-library/react';
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

function emitLog(
  socket: MockWebSocket,
  index: number,
  level: LogLevel = LogLevel.INFO,
) {
  const message = create(WsMessageSchema, {
    eventType: EventType.LOG,
    payload: {
      case: 'log',
      value: create(LogEventSchema, {
        timestampMs: BigInt(1_700_000_000_000 + index),
        level,
        target: 'test',
        message: `log-${index}`,
      }),
    },
  });
  socket.emitBinary(toBinary(WsMessageSchema, message));
}

function renderViewer() {
  const utils = renderWithProviders(<LogViewer />);
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

  it('keeps only the newest entries once past the cap', () => {
    const { container, socket } = renderViewer();
    // Only the ten oldest and ten newest frames are warnings, so the warning
    // filter shows which end survived without rendering hundreds of rows.
    // The select is driven synchronously: the async query helpers never
    // resolve under fake timers.
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Log level' }), {
      key: 'ArrowDown',
    });
    fireEvent.click(screen.getByRole('option', { name: 'WARN+' }));
    const total = MAX_LOG_ENTRIES + 20;
    act(() => {
      for (let i = 0; i < total; i++) {
        const edge = i < 10 || i >= total - 10;
        emitLog(socket, i, edge ? LogLevel.WARN : LogLevel.INFO);
      }
    });
    flush();

    expect(renderedRowCount(container)).toBe(10);
    expect(screen.queryByText('log-9')).not.toBeInTheDocument();
    expect(screen.getByText(`log-${total - 10}`)).toBeInTheDocument();
    expect(screen.getByText(`log-${total - 1}`)).toBeInTheDocument();
  });

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
