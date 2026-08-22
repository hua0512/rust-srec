import { act, render, waitFor } from '@testing-library/react';
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
    readonly video = { pause: vi.fn() };
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
    />
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

  it('requires a desktop token before proxying authenticated sources', () => {
    const options = {
      url: 'https://media.example/video.mp4',
      headers: { Authorization: 'upstream' },
      desktopBuild: true,
      baseUrl: 'http://localhost:12555/api/',
    };

    expect(buildPlaybackUrl({ ...options, desktopToken: null })).toBe(
      options.url,
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
