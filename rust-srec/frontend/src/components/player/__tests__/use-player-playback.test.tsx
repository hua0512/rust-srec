import { act, fireEvent, render, waitFor } from '@testing-library/react';
import {
  buildPlaybackUrl,
  usePlayerPlayback,
  type UsePlayerPlaybackOptions,
} from '../use-player-playback';

const artplayerMock = vi.hoisted(() => {
  type EventHandler = (...args: unknown[]) => void;

  class MockArtplayer {
    static FULLSCREEN_WEB_IN_BODY = true;
    readonly events = new Map<string, EventHandler[]>();
    readonly video = {
      pause: vi.fn(),
      paused: true,
      readyState: 0,
      error: null as { code: number } | null,
    };
    readonly destroy = vi.fn();
    volume: number;
    muted: boolean;
    fullscreenWeb = false;
    isReady = false;

    constructor(readonly options: Record<string, unknown>) {
      this.volume = typeof options.volume === 'number' ? options.volume : 0.5;
      this.muted = typeof options.muted === 'boolean' ? options.muted : false;
      instances.push(this);
    }

    on(event: string, handler: EventHandler) {
      const handlers = this.events.get(event) ?? [];
      handlers.push(handler);
      this.events.set(event, handlers);
    }

    emit(event: string, ...args: unknown[]) {
      for (const handler of this.events.get(event) ?? []) handler(...args);
    }
  }

  const instances: MockArtplayer[] = [];
  return { MockArtplayer, instances };
});

const resolveUrlMock = vi.hoisted(() => vi.fn());
const toastErrorMock = vi.hoisted(() => vi.fn());

vi.mock('artplayer', () => ({ default: artplayerMock.MockArtplayer }));
vi.mock('@/server/functions/parse', () => ({ resolveUrl: resolveUrlMock }));
vi.mock('sonner', () => ({ toast: { error: toastErrorMock } }));
vi.mock('@/utils/desktop', () => ({ isDesktopBuild: () => false }));
vi.mock('@/utils/env', () => ({ BASE_URL: '/api' }));
vi.mock('@/utils/session', () => ({ getDesktopAccessToken: () => null }));

const defaultOptions: UsePlayerPlaybackOptions = {
  url: 'https://media.example/recording.mp4',
  title: 'recording.mp4',
  muted: false,
  volume: 0.5,
  defaultWebFullscreen: false,
  mediaType: 'mp4',
  isLive: false,
};

function PlaybackHarness(options: UsePlayerPlaybackOptions) {
  const playback = usePlayerPlayback(options);
  return (
    <div
      ref={playback.containerRef}
      data-error={playback.error ?? ''}
      data-loading={String(playback.loading)}
      data-status={playback.status}
      data-connection={playback.connection}
    >
      <button onClick={playback.reload}>Retry</button>
    </div>
  );
}

describe('buildPlaybackUrl', () => {
  it('only proxies sources that need request headers', () => {
    const directUrl = buildPlaybackUrl({
      url: 'https://media.example/video.mp4',
      desktopBuild: false,
      desktopToken: null,
      baseUrl: '/api',
    });
    const proxiedUrl = buildPlaybackUrl({
      url: 'https://media.example/video.mp4',
      headers: { Referer: 'https://source.example/' },
      desktopBuild: false,
      desktopToken: null,
      baseUrl: '/api',
    });

    expect(directUrl).toBe('https://media.example/video.mp4');
    expect(proxiedUrl).toContain('/stream-proxy?url=');
    expect(decodeURIComponent(proxiedUrl)).toContain(
      '"Referer":"https://source.example/"',
    );
  });

  it('can force the proxy for header-free sources on web and desktop', () => {
    const options = {
      url: 'https://media.example/live.m3u8?key=source',
      connectionMode: 'proxy' as const,
      desktopToken: 'session',
      baseUrl: 'http://localhost:12555/api/',
    };
    expect(buildPlaybackUrl({ ...options, desktopBuild: false })).toContain(
      '/stream-proxy?url=',
    );
    const desktopUrl = new URL(
      buildPlaybackUrl({ ...options, desktopBuild: true }),
    );
    expect(desktopUrl.pathname).toBe('/api/stream-proxy');
    expect(desktopUrl.searchParams.get('token')).toBe('session');
    expect(desktopUrl.searchParams.get('url')).toBe(options.url);
  });

  it('never silently drops required headers in direct mode', () => {
    expect(() =>
      buildPlaybackUrl({
        url: 'https://media.example/live',
        headers: { Cookie: 'secret' },
        connectionMode: 'direct',
        desktopBuild: false,
        desktopToken: null,
        baseUrl: '/api',
      }),
    ).toThrow('headers');
  });

  it('requires a desktop token before proxying authenticated sources', () => {
    const options = {
      url: 'https://media.example/video.mp4',
      headers: { Authorization: 'upstream' },
      desktopBuild: true,
      baseUrl: 'http://localhost:12555/api/',
    };

    expect(() => buildPlaybackUrl({ ...options, desktopToken: null })).toThrow(
      'session',
    );
    expect(
      buildPlaybackUrl({ ...options, desktopToken: 'desktop-token' }),
    ).toContain(
      'http://localhost:12555/api/stream-proxy?url=https%3A%2F%2Fmedia.example',
    );
  });
});

describe('usePlayerPlayback', () => {
  beforeEach(() => {
    artplayerMock.instances.length = 0;
    artplayerMock.MockArtplayer.FULLSCREEN_WEB_IN_BODY = true;
    resolveUrlMock.mockReset();
    toastErrorMock.mockReset();
  });

  it('updates mutable player state and callbacks without rebuilding', async () => {
    const firstVolumeCallback = vi.fn();
    const latestVolumeCallback = vi.fn();
    const muteCallback = vi.fn();
    const view = render(
      <PlaybackHarness
        {...defaultOptions}
        defaultWebFullscreen
        onVolumeChange={firstVolumeCallback}
      />,
    );

    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    const player = artplayerMock.instances[0];
    expect(player).toBeDefined();
    expect(artplayerMock.MockArtplayer.FULLSCREEN_WEB_IN_BODY).toBe(false);

    player!.isReady = true;
    act(() => player!.emit('ready'));
    expect(player!.fullscreenWeb).toBe(true);

    view.rerender(
      <PlaybackHarness
        {...defaultOptions}
        muted
        volume={0.8}
        onVolumeChange={latestVolumeCallback}
        onMuteChange={muteCallback}
      />,
    );

    await waitFor(() => {
      expect(player!.volume).toBe(0.8);
      expect(player!.muted).toBe(true);
    });
    expect(artplayerMock.instances).toHaveLength(1);

    act(() => player!.emit('video:volumechange'));
    expect(firstVolumeCallback).not.toHaveBeenCalled();
    expect(latestVolumeCallback).toHaveBeenCalledWith(0.8);
    expect(muteCallback).toHaveBeenCalledWith(true);

    view.unmount();
    expect(player!.video.pause).toHaveBeenCalledOnce();
    expect(player!.destroy).toHaveBeenCalledWith(false);
  });

  it('tears down the old player while source resolution is pending', async () => {
    let finishResolution: ((response: unknown) => void) | undefined;
    resolveUrlMock.mockImplementation(
      () =>
        new Promise((resolve) => {
          finishResolution = resolve;
        }),
    );
    const view = render(<PlaybackHarness {...defaultOptions} />);

    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    const initialPlayer = artplayerMock.instances[0];
    const streamData = { platform: 'example' };
    const headers = { Referer: 'https://source.example/' };
    view.rerender(
      <PlaybackHarness
        {...defaultOptions}
        url="https://media.example/fallback.mp4"
        title="resolved.mp4"
        headers={headers}
        streamData={streamData}
      />,
    );

    await waitFor(() => expect(resolveUrlMock).toHaveBeenCalledOnce());
    await waitFor(() => expect(initialPlayer!.destroy).toHaveBeenCalled());
    expect(artplayerMock.instances).toHaveLength(1);

    await act(async () => {
      finishResolution?.({
        success: true,
        stream_info: { url: 'https://media.example/resolved.mp4' },
      });
      await Promise.resolve();
    });

    await waitFor(() => expect(artplayerMock.instances).toHaveLength(2));
    const resolvedPlayer = artplayerMock.instances[1];
    const playbackUrl = resolvedPlayer!.options.url;
    expect(typeof playbackUrl).toBe('string');
    expect(decodeURIComponent(playbackUrl as string)).toContain(
      'https://media.example/resolved.mp4',
    );
    expect(decodeURIComponent(playbackUrl as string)).toContain(
      '"Referer":"https://source.example/"',
    );
  });
});

describe('playback feedback and recovery', () => {
  beforeEach(() => {
    artplayerMock.instances.length = 0;
    resolveUrlMock.mockReset();
  });

  it('tracks actual playback and buffering, including pause and end', async () => {
    const view = render(<PlaybackHarness {...defaultOptions} />);
    const state = view.container.firstElementChild!;
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    const player = artplayerMock.instances[0]!;
    expect(state).toHaveAttribute('data-status', 'connecting');
    act(() => player.emit('ready'));
    expect(state).toHaveAttribute('data-status', 'ready');
    for (const [event, status] of [
      ['playing', 'playing'],
      ['waiting', 'buffering'],
      ['playing', 'playing'],
      ['pause', 'paused'],
      ['ended', 'ended'],
    ]) {
      act(() => player.emit(`video:${event}`));
      expect(state).toHaveAttribute('data-status', status);
      expect(state).toHaveAttribute(
        'data-loading',
        String(status === 'buffering'),
      );
    }
    player.video.paused = true;
    act(() => player.emit('video:stalled'));
    expect(state).toHaveAttribute('data-status', 'ended');
  });

  it('shows a safe error and retries with a new player', async () => {
    const view = render(<PlaybackHarness {...defaultOptions} />);
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    const player = artplayerMock.instances[0]!;
    player.video.error = { code: 3 };
    act(() =>
      player.emit('error', new Error('https://media.example/?token=secret')),
    );
    expect(view.container.firstElementChild).toHaveAttribute(
      'data-error',
      'media',
    );
    expect(view.container.innerHTML).not.toContain('secret');
    fireEvent.click(view.getByText('Retry'));
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(2));
    expect(player.destroy).toHaveBeenCalledOnce();
    expect(view.container.firstElementChild).toHaveAttribute('data-error', '');
    act(() => player.emit('video:playing'));
    expect(view.container.firstElementChild).toHaveAttribute(
      'data-status',
      'connecting',
    );
  });

  it('keeps display titles separate from resolution URLs and passes source cookies', async () => {
    resolveUrlMock.mockResolvedValue({
      success: true,
      stream_info: { url: 'https://media.example/fresh.mp4' },
    });
    const streamData = { url: 'https://media.example/old.mp4' };
    render(
      <PlaybackHarness
        {...defaultOptions}
        sourceUrl="https://source.example/channel"
        title="Creator's live stream"
        streamData={streamData}
        headers={{ Cookie: 'credential' }}
      />,
    );
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    expect(resolveUrlMock).toHaveBeenCalledWith({
      data: {
        url: 'https://source.example/channel',
        stream_info: streamData,
        cookies: 'credential',
      },
    });
  });

  it('stops on resolution failure instead of silently playing an expired URL', async () => {
    resolveUrlMock.mockResolvedValueOnce({
      success: false,
      error: 'signed URL secret',
    });
    const view = render(
      <PlaybackHarness
        {...defaultOptions}
        streamData={{ url: defaultOptions.url }}
      />,
    );
    await waitFor(() =>
      expect(view.container.firstElementChild).toHaveAttribute(
        'data-error',
        'resolution',
      ),
    );
    expect(artplayerMock.instances).toHaveLength(0);
    resolveUrlMock.mockResolvedValueOnce({
      success: true,
      stream_info: { url: 'https://media.example/fresh.mp4' },
    });
    fireEvent.click(view.getByText('Retry'));
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    expect(artplayerMock.instances[0]!.options.url).toBe(
      'https://media.example/fresh.mp4',
    );
  });

  it('rebuilds when switching from direct to server proxy', async () => {
    const view = render(<PlaybackHarness {...defaultOptions} />);
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(1));
    view.rerender(
      <PlaybackHarness {...defaultOptions} connectionMode="proxy" />,
    );
    await waitFor(() => expect(artplayerMock.instances).toHaveLength(2));
    expect(artplayerMock.instances[0]!.destroy).toHaveBeenCalledOnce();
    expect(artplayerMock.instances[1]!.options.url).toContain(
      '/stream-proxy?url=',
    );
    expect(view.container.firstElementChild).toHaveAttribute(
      'data-connection',
      'proxy',
    );
  });
});
