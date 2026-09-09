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
import { create, toBinary } from '@bufbuild/protobuf';

import {
  EventType,
  LogEventSchema,
  LogLevel,
  WsMessageSchema,
} from '@/api/proto/gen/log_event_pb.js';
import { LogViewer } from '../log-viewer';

const FLUSH_WINDOW_MS = 50;
const MAX_LOG_ENTRIES = 500;

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

  emitLog(index: number) {
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
    const bytes = toBinary(WsMessageSchema, message);
    this.onmessage?.(
      new MessageEvent('message', {
        data: bytes.buffer.slice(
          bytes.byteOffset,
          bytes.byteOffset + bytes.byteLength,
        ),
      }),
    );
  }
}

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

function renderViewer() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  const utils = render(
    <QueryClientProvider client={queryClient}>
      <I18nProvider i18n={i18n}>
        <LogViewer />
      </I18nProvider>
    </QueryClientProvider>,
  );
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
    for (let i = 0; i < count; i++) socket.emitLog(offset + i);
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
    routeContext.user = sessionWithToken('token-a');
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

    emitLogs(socket, MAX_LOG_ENTRIES + 20);
    flush();

    expect(renderedRowCount(container)).toBe(MAX_LOG_ENTRIES);
    expect(screen.queryByText('log-0')).not.toBeInTheDocument();
    expect(screen.getByText('log-519')).toBeInTheDocument();
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
