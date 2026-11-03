import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, render } from '@testing-library/react';
import { sessionQueryOptions } from '@/api/session';
import type { ClientSessionData } from '@/utils/session';
import type { LiveDanmuItem } from '../danmu-overlay';
import { useLiveDanmu } from '../use-live-danmu';

vi.mock('@/server/functions/auth', () => ({ checkAuthFn: vi.fn() }));

class MockWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 3;
  static instances: MockWebSocket[] = [];

  readonly url: string;
  readyState = MockWebSocket.CONNECTING;
  binaryType: BinaryType = 'blob';
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  constructor(url: string | URL) {
    this.url = url.toString();
    MockWebSocket.instances.push(this);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
  }

  emitClose(code: number) {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent('close', { code }));
  }
}

function Harness({
  handle,
  onItems,
  onExpired,
}: {
  handle: string;
  onItems: (items: LiveDanmuItem[]) => void;
  onExpired: () => void;
}) {
  useLiveDanmu(handle, onItems, onExpired);
  return null;
}

function renderWithSession(handle: string) {
  const queryClient = new QueryClient();
  const session: ClientSessionData = {
    username: 'user',
    token: {
      access_token: 'token',
      expires_in: Date.now() + 10 * 60_000,
      refresh_expires_in: Date.now() + 60 * 60_000,
    },
    roles: [],
    mustChangePassword: false,
  };
  queryClient.setQueryData(sessionQueryOptions.queryKey, session);
  const onItems = vi.fn();
  const onExpired = vi.fn();
  const view = render(
    <QueryClientProvider client={queryClient}>
      <Harness handle={handle} onItems={onItems} onExpired={onExpired} />
    </QueryClientProvider>,
  );
  const rerender = (next: string) =>
    view.rerender(
      <QueryClientProvider client={queryClient}>
        <Harness handle={next} onItems={onItems} onExpired={onExpired} />
      </QueryClientProvider>,
    );
  return { onItems, onExpired, rerender };
}

describe('useLiveDanmu', () => {
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

  it('passes on valid comments and skips malformed frames', () => {
    const { onItems } = renderWithSession('h1');
    const [socket] = MockWebSocket.instances;
    expect(socket.url).toMatch(/\/parse\/danmu\/ws\?handle=h1&token=token$/);

    const item = { text: 'hi', color: 0xff0000, kind: 'chat', user: 'a' };
    act(() => {
      for (const data of [
        'not json',
        JSON.stringify({ type: 'unknown', items: [item] }),
        JSON.stringify({ type: 'batch', items: [item, { text: 1 }] }),
      ]) {
        socket.onmessage?.(new MessageEvent('message', { data }));
      }
    });

    expect(onItems).toHaveBeenCalledExactlyOnceWith([item]);
  });

  it('asks for a renewed handle when it expires and connects with it', () => {
    const { onExpired, rerender } = renderWithSession('h1');
    act(() => MockWebSocket.instances[0].emitClose(4404));
    act(() => {
      vi.advanceTimersByTime(30_000);
    });
    expect(onExpired).toHaveBeenCalledOnce();
    expect(MockWebSocket.instances).toHaveLength(1);

    rerender('h2');
    expect(MockWebSocket.instances).toHaveLength(2);
    expect(MockWebSocket.instances[1].url).toContain('handle=h2');
  });

  it('retries a full server slowly and any other drop promptly', () => {
    renderWithSession('h1');
    act(() => MockWebSocket.instances[0].emitClose(4429));
    act(() => {
      vi.advanceTimersByTime(14_000);
    });
    expect(MockWebSocket.instances).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    expect(MockWebSocket.instances).toHaveLength(2);

    act(() => MockWebSocket.instances[1].emitClose(1006));
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(MockWebSocket.instances).toHaveLength(3);
  });
});
