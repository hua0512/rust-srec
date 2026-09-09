import { act, cleanup, render } from '@testing-library/react';

import { useAuthedWebSocket } from '../use-authed-websocket';

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
  closed = false;

  constructor(url: string | URL) {
    this.url = url.toString();
    MockWebSocket.instances.push(this);
  }

  close() {
    this.closed = true;
    this.readyState = MockWebSocket.CLOSED;
  }

  emitOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event('open'));
  }

  emitClose() {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent('close'));
  }

  emitMessage(data: string) {
    this.onmessage?.(new MessageEvent('message', { data }));
  }
}

interface HarnessProps {
  accessToken: string | undefined;
  onMessage?: (event: MessageEvent) => void;
  onDisconnect?: () => void;
}

function Harness({ accessToken, onMessage, onDisconnect }: HarnessProps) {
  const { status } = useAuthedWebSocket({
    accessToken,
    path: '/logging/stream',
    onMessage: onMessage ?? (() => {}),
    onDisconnect,
  });
  return <span data-testid="status">{status}</span>;
}

const BASE_DELAY_MS = 1000;
const MAX_DELAY_MS = 30_000;

describe('useAuthedWebSocket', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances = [];
    vi.stubGlobal('WebSocket', MockWebSocket);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('opens one socket and reports its status', () => {
    const { getByTestId } = render(<Harness accessToken="token-a" />);

    expect(MockWebSocket.instances).toHaveLength(1);
    expect(MockWebSocket.instances[0].url).toContain('/logging/stream');
    expect(getByTestId('status')).toHaveTextContent('connecting');

    act(() => MockWebSocket.instances[0].emitOpen());
    expect(getByTestId('status')).toHaveTextContent('connected');
  });

  it('never opens a second socket beside a live one', () => {
    const { rerender } = render(<Harness accessToken="token-a" />);
    act(() => MockWebSocket.instances[0].emitOpen());

    // A re-render with the same session must not start another handshake.
    rerender(<Harness accessToken="token-a" />);
    act(() => {
      vi.advanceTimersByTime(MAX_DELAY_MS);
    });

    expect(MockWebSocket.instances).toHaveLength(1);
  });

  it('backs off exponentially between reconnect attempts', () => {
    render(<Harness accessToken="token-a" />);
    act(() => MockWebSocket.instances[0].emitOpen());

    // First drop: one second.
    act(() => MockWebSocket.instances[0].emitClose());
    act(() => {
      vi.advanceTimersByTime(BASE_DELAY_MS - 1);
    });
    expect(MockWebSocket.instances).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(MockWebSocket.instances).toHaveLength(2);

    // Second drop without a successful handshake in between: two seconds.
    act(() => MockWebSocket.instances[1].emitClose());
    act(() => {
      vi.advanceTimersByTime(BASE_DELAY_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(2);
    act(() => {
      vi.advanceTimersByTime(BASE_DELAY_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(3);

    // A successful handshake puts the next drop back at one second.
    act(() => MockWebSocket.instances[2].emitOpen());
    act(() => MockWebSocket.instances[2].emitClose());
    act(() => {
      vi.advanceTimersByTime(BASE_DELAY_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(4);
  });

  it('reconnects immediately when a renewed token arrives during backoff', () => {
    const { rerender } = render(<Harness accessToken="token-a" />);
    act(() => MockWebSocket.instances[0].emitOpen());
    act(() => MockWebSocket.instances[0].emitClose());

    // Renewed mid-backoff: waiting out a timer that would hand the handshake an
    // expired token buys nothing.
    act(() => {
      rerender(<Harness accessToken="token-b" />);
    });

    expect(MockWebSocket.instances).toHaveLength(2);
    expect(MockWebSocket.instances[1].url).toContain('token-b');

    // The cleared timer must not fire a third socket afterwards.
    act(() => {
      vi.advanceTimersByTime(MAX_DELAY_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it('leaves a healthy socket alone when the token is renewed', () => {
    const { rerender } = render(<Harness accessToken="token-a" />);
    const socket = MockWebSocket.instances[0];
    act(() => socket.emitOpen());

    act(() => {
      rerender(<Harness accessToken="token-b" />);
      vi.advanceTimersByTime(MAX_DELAY_MS);
    });

    expect(MockWebSocket.instances).toHaveLength(1);
    expect(socket.closed).toBe(false);
  });

  it('closes for good when the session ends and reports it once', () => {
    const onDisconnect = vi.fn();
    const { rerender } = render(
      <Harness accessToken="token-a" onDisconnect={onDisconnect} />,
    );
    const socket = MockWebSocket.instances[0];
    act(() => socket.emitOpen());

    act(() => {
      rerender(<Harness accessToken={undefined} onDisconnect={onDisconnect} />);
    });

    expect(socket.closed).toBe(true);
    expect(onDisconnect).toHaveBeenCalledTimes(1);

    // An intentional close does not schedule a reconnect.
    act(() => {
      socket.emitClose();
      vi.advanceTimersByTime(MAX_DELAY_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  it('closes the socket and drops pending reconnects on unmount', () => {
    const { unmount } = render(<Harness accessToken="token-a" />);
    const socket = MockWebSocket.instances[0];
    act(() => socket.emitOpen());
    act(() => socket.emitClose());

    unmount();

    act(() => {
      vi.advanceTimersByTime(MAX_DELAY_MS);
    });
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  it('ignores events from a socket that is no longer current', () => {
    const onMessage = vi.fn();
    const { rerender } = render(
      <Harness accessToken="token-a" onMessage={onMessage} />,
    );
    const staleSocket = MockWebSocket.instances[0];
    act(() => staleSocket.emitOpen());
    act(() => staleSocket.emitClose());
    act(() => {
      rerender(<Harness accessToken="token-b" onMessage={onMessage} />);
    });
    expect(MockWebSocket.instances).toHaveLength(2);

    act(() => {
      staleSocket.emitMessage('late frame');
      staleSocket.emitClose();
      vi.advanceTimersByTime(MAX_DELAY_MS);
    });

    expect(MockWebSocket.instances).toHaveLength(2);
    expect(onMessage).not.toHaveBeenCalled();
  });
});
