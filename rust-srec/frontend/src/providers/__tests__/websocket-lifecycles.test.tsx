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
import { WebSocketProvider } from '../WebSocketProvider';

const routeContext = vi.hoisted(() => ({
  user: { token: { access_token: 'token-a' } },
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
    queryClient.setQueryData(['session'], {
      token: { access_token: 'token-b' },
    });
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(MockWebSocket.instances).toHaveLength(2);
}

describe('WebSocket lifecycle ownership', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances = [];
    routeContext.user = { token: { access_token: 'token-a' } };
    vi.stubGlobal('WebSocket', MockWebSocket);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('ignores a stale provider socket close after token rotation', async () => {
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <WebSocketProvider>
          <div>child</div>
        </WebSocketProvider>
      </QueryClientProvider>,
    );

    const staleSocket = MockWebSocket.instances[0];
    await rotateToken(queryClient);

    act(() => {
      staleSocket.emitClose();
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
