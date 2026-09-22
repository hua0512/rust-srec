import { useCallback, useEffect, useRef, useState } from 'react';
import { resolvePlayerMediaType, type PlayerMediaType } from '@/lib/media';
import { resolveUrl } from '@/server/functions/parse';
import { isDesktopBuild } from '@/utils/desktop';
import { BASE_URL } from '@/utils/env';
import { getDesktopAccessToken } from '@/utils/session';
import { MpegtsPlaybackController } from './mpegts-playback';
import {
  classifyPlaybackError,
  effectiveConnection,
  PlaybackConfigurationError,
  type ConnectionMode,
  type PlaybackError,
  type PlaybackStatus,
} from './playback-state';

type ArtplayerInstance = InstanceType<(typeof import('artplayer'))['default']>;
type ArtplayerOptions = ConstructorParameters<
  (typeof import('artplayer'))['default']
>[0];
type ArtplayerMediaType = NonNullable<ArtplayerOptions['type']>;
type HlsInstance = InstanceType<(typeof import('hls.js'))['default']>;

interface PlaybackSource {
  url: string;
  headers?: Record<string, string>;
}

interface SourceRequest {
  url: string;
  headers?: Record<string, string>;
  title: string;
  streamData: unknown;
  reloadKey: number;
}

interface ResolvedSource {
  request: SourceRequest;
  source: PlaybackSource | null;
  error: PlaybackError | null;
}

interface UseResolvedSourceOptions {
  url: string;
  headers?: Record<string, string>;
  title?: string;
  streamData?: unknown;
  reloadKey: number;
}

export interface UsePlayerPlaybackOptions {
  connectionMode?: ConnectionMode;
  sourceUrl?: string;
  url: string;
  headers?: Record<string, string>;
  title?: string;
  streamData?: unknown;
  muted: boolean;
  volume: number;
  onVolumeChange?: (volume: number) => void;
  onMuteChange?: (muted: boolean) => void;
  defaultWebFullscreen: boolean;
  mediaType?: string;
  isLive: boolean;
  mediaDurationSecs?: number | null;
  mediaFileSizeBytes?: number;
}

export interface BuildPlaybackUrlOptions extends PlaybackSource {
  connectionMode?: ConnectionMode;
  desktopBuild: boolean;
  desktopToken: string | null;
  baseUrl: string;
}

function getArtplayerType(mediaType: PlayerMediaType): ArtplayerMediaType {
  switch (mediaType) {
    case 'hls':
      return 'm3u8';
    case 'flv':
      return 'flv';
    case 'mpegts':
      return 'mpegts';
    case 'mp4':
      return 'mp4';
    case 'mkv':
      return 'mkv';
    case 'audio':
      return 'mp3';
    case 'native':
    case 'auto':
      return 'auto';
  }
}

export function buildPlaybackUrl({
  url,
  headers,
  desktopBuild,
  desktopToken,
  baseUrl,
  connectionMode = 'auto',
}: BuildPlaybackUrlOptions): string {
  const hasHeaders = Object.keys(headers ?? {}).length > 0;
  if (connectionMode === 'direct' && hasHeaders) {
    throw new PlaybackConfigurationError('headers');
  }
  if (effectiveConnection(connectionMode, headers) === 'direct') return url;

  const query = `url=${encodeURIComponent(url)}&headers=${encodeURIComponent(JSON.stringify(headers ?? {}))}`;
  if (!desktopBuild) return `/stream-proxy?${query}`;
  if (!desktopToken) throw new PlaybackConfigurationError('session');

  return `${baseUrl.replace(/\/$/, '')}/stream-proxy?${query}&token=${encodeURIComponent(desktopToken)}`;
}

function matchesRequest(
  resolved: ResolvedSource | null,
  options: UseResolvedSourceOptions,
): resolved is ResolvedSource {
  if (!resolved || !options.title || !options.streamData) return false;

  const { request } = resolved;
  return (
    request.url === options.url &&
    request.headers === options.headers &&
    request.title === options.title &&
    request.streamData === options.streamData &&
    request.reloadKey === options.reloadKey
  );
}

function useResolvedSource(options: UseResolvedSourceOptions): {
  source: PlaybackSource | null;
  resolving: boolean;
  error: PlaybackError | null;
} {
  const { url, headers, title, streamData, reloadKey } = options;
  const [resolved, setResolved] = useState<ResolvedSource | null>(null);
  const needsResolution = Boolean(streamData && title);

  useEffect(() => {
    if (!streamData || !title) return;

    let disposed = false;
    const request: SourceRequest = {
      url,
      headers,
      title,
      streamData,
      reloadKey,
    };

    const resolve = async () => {
      try {
        const response = await resolveUrl({
          data: {
            url: title,
            stream_info: streamData,
            cookies: Object.entries(headers ?? {}).find(
              ([name]) => name.toLowerCase() === 'cookie',
            )?.[1],
          },
        });
        if (disposed) return;
        if (!response.success || !response.stream_info?.url) {
          setResolved({ request, source: null, error: 'resolution' });
          return;
        }
        setResolved({
          request,
          source: { url: response.stream_info.url, headers },
          error: null,
        });
      } catch {
        if (!disposed)
          setResolved({ request, source: null, error: 'resolution' });
      }
    };

    void resolve();
    return () => {
      disposed = true;
    };
  }, [url, headers, title, streamData, reloadKey]);

  if (!needsResolution) {
    return { source: { url, headers }, resolving: false, error: null };
  }
  if (!matchesRequest(resolved, options)) {
    return { source: null, resolving: true, error: null };
  }
  return { source: resolved.source, resolving: false, error: resolved.error };
}

export function usePlayerPlayback(options: UsePlayerPlaybackOptions) {
  const {
    url,
    headers,
    title,
    streamData,
    muted,
    volume,
    onVolumeChange,
    onMuteChange,
    defaultWebFullscreen,
    mediaType,
    isLive,
    mediaDurationSecs,
    mediaFileSizeBytes,
    sourceUrl,
    connectionMode = 'auto',
  } = options;
  const containerRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<ArtplayerInstance | null>(null);
  const volumeRef = useRef(volume);
  const mutedRef = useRef(muted);
  const defaultWebFullscreenRef = useRef(defaultWebFullscreen);
  const onVolumeChangeRef = useRef(onVolumeChange);
  const onMuteChangeRef = useRef(onMuteChange);
  const [error, setError] = useState<PlaybackError | null>(null);
  const [status, setStatus] = useState<PlaybackStatus>('connecting');
  const [reloadKey, setReloadKey] = useState(0);

  volumeRef.current = volume;
  mutedRef.current = muted;
  defaultWebFullscreenRef.current = defaultWebFullscreen;
  onVolumeChangeRef.current = onVolumeChange;
  onMuteChangeRef.current = onMuteChange;

  const {
    source,
    resolving,
    error: resolutionError,
  } = useResolvedSource({
    url,
    headers,
    title: sourceUrl ?? title,
    streamData,
    reloadKey,
  });
  const desktopBuild = isDesktopBuild();
  const desktopToken = desktopBuild ? getDesktopAccessToken() : null;
  const connection = effectiveConnection(
    connectionMode,
    source?.headers ?? headers,
  );
  let playUrl: string | null = null;
  let configurationError: PlaybackError | null = null;
  if (source) {
    try {
      playUrl = buildPlaybackUrl({
        ...source,
        connectionMode,
        desktopBuild,
        desktopToken,
        baseUrl: BASE_URL,
      });
    } catch (error) {
      configurationError =
        error instanceof PlaybackConfigurationError ? error.reason : 'unknown';
    }
  }
  const resolvedMediaType = source
    ? resolvePlayerMediaType(mediaType, source.url, title)
    : null;

  const reload = useCallback(() => {
    setError(null);
    setStatus('connecting');
    setReloadKey((current) => current + 1);
  }, []);

  useEffect(() => {
    if (!resolving) return;
    setError(null);
    setStatus('connecting');
  }, [resolving]);

  useEffect(() => {
    const player = playerRef.current;
    if (player && player.volume !== volume) player.volume = volume;
  }, [volume]);

  useEffect(() => {
    const player = playerRef.current;
    if (player && player.muted !== muted) player.muted = muted;
  }, [muted]);

  useEffect(() => {
    if (defaultWebFullscreen && playerRef.current?.isReady) {
      playerRef.current.fullscreenWeb = true;
    }
  }, [defaultWebFullscreen]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !playUrl || !resolvedMediaType) return;

    let disposed = false;
    let art: ArtplayerInstance | null = null;
    let hls: HlsInstance | null = null;
    let mpegtsController: MpegtsPlaybackController | null = null;

    const destroySession = () => {
      hls?.destroy();
      hls = null;
      mpegtsController?.destroy();
      mpegtsController = null;

      const currentArt = art;
      art = null;
      if (currentArt) {
        currentArt.video.pause();
        currentArt.destroy(false);
        if (playerRef.current === currentArt) playerRef.current = null;
      }
      container.replaceChildren();
    };

    const initialize = async () => {
      setError(null);
      setStatus('connecting');

      try {
        const { default: Artplayer } = await import('artplayer');
        if (disposed) return;

        // Artplayer 5.4 reparents web-fullscreen players into document.body by
        // default, which can leave Chromium's MediaSource video surface black.
        Artplayer.FULLSCREEN_WEB_IN_BODY = false;

        const artplayerType = getArtplayerType(resolvedMediaType);
        const options: ArtplayerOptions = {
          container,
          url: playUrl,
          autoplay: true,
          volume: volumeRef.current,
          muted: mutedRef.current,
          autoSize: false,
          pip: true,
          mutex: false,
          setting: true,
          playbackRate: true,
          aspectRatio: true,
          fullscreen: true,
          fullscreenWeb: true,
          isLive,
          miniProgressBar: !isLive,
          theme: '#3b82f6',
          type: artplayerType,
        };

        if (resolvedMediaType === 'hls') {
          const { default: Hls } = await import('hls.js');
          if (disposed) return;

          if (Hls.isSupported()) {
            options.customType = {
              m3u8: (video: HTMLVideoElement, sourceUrl: string) => {
                hls = new Hls({
                  enableWorker: true,
                  lowLatencyMode: isLive,
                });
                hls.loadSource(sourceUrl);
                hls.attachMedia(video);
                hls.on(Hls.Events.ERROR, (_event, data) => {
                  if (!disposed && data.fatal) {
                    setError(
                      classifyPlaybackError(
                        data.type,
                        data.response?.code,
                        connection === 'proxy',
                      ),
                    );
                    setStatus('ready');
                  }
                });
              },
            };
          }
        }

        const mpegtsType =
          resolvedMediaType === 'flv' || resolvedMediaType === 'mpegts'
            ? resolvedMediaType
            : null;
        if (mpegtsType) {
          const { default: mpegts } = await import('mpegts.js');
          if (disposed) return;
          if (!mpegts.isSupported()) {
            throw new PlaybackConfigurationError('unsupported');
          }

          mpegtsController = new MpegtsPlaybackController(mpegts, {
            mediaType: mpegtsType,
            isLive,
            durationSecs: mediaDurationSecs,
            fileSizeBytes: mediaFileSizeBytes,
            onLoadingChange: (nextLoading) => {
              if (!disposed)
                setStatus(
                  nextLoading
                    ? 'buffering'
                    : playerRef.current?.video.paused
                      ? 'paused'
                      : 'playing',
                );
            },
            onError: ({ type, details, data }) => {
              if (!disposed) {
                const code =
                  data &&
                  typeof data === 'object' &&
                  'code' in data &&
                  typeof data.code === 'number'
                    ? data.code
                    : undefined;
                setError(
                  classifyPlaybackError(
                    `${type} ${details}`,
                    code,
                    connection === 'proxy',
                  ),
                );
              }
            },
            onStalled: () => {
              if (!disposed) {
                setError('stalled');
              }
            },
            onWarning: (message) => {
              // Engine error objects may contain signed URLs.
              console.warn(message);
            },
          });
          options.customType = {
            [artplayerType]: (video: HTMLVideoElement, sourceUrl: string) => {
              mpegtsController?.attach(video, sourceUrl);
            },
          };
        }

        if (disposed) return;
        const createdArt = new Artplayer(options);
        art = createdArt;
        if (disposed) {
          destroySession();
          return;
        }
        playerRef.current = createdArt;

        createdArt.on('ready', () => {
          if (disposed) return;
          if (defaultWebFullscreenRef.current) {
            createdArt.fullscreenWeb = true;
          }
          setStatus(createdArt.video.paused ? 'ready' : 'playing');
        });
        createdArt.on('error', (playerError: Error) => {
          if (disposed) return;
          mpegtsController?.cancelSeekRecovery();
          const mediaError = createdArt.video.error;
          setError(
            (current) =>
              current ??
              (mediaError?.code === 4
                ? 'unsupported'
                : mediaError?.code === 3
                  ? 'media'
                  : mediaError?.code === 2
                    ? 'network'
                    : classifyPlaybackError(playerError?.name ?? 'unknown')),
          );
          setStatus('ready');
        });
        createdArt.on('video:volumechange', () => {
          onVolumeChangeRef.current?.(createdArt.volume);
          onMuteChangeRef.current?.(createdArt.muted);
        });
        createdArt.on('seek', (currentTime) => {
          if (!mpegtsController?.seek(currentTime)) return;
          setError(null);
        });

        const updateStatus = (nextStatus: PlaybackStatus) => {
          if (disposed) return;
          setStatus(nextStatus);
          if (nextStatus === 'playing') setError(null);
        };
        createdArt.on('video:playing', () => updateStatus('playing'));
        createdArt.on('video:waiting', () => updateStatus('buffering'));
        createdArt.on('video:stalled', () => {
          if (!createdArt.video.paused && createdArt.video.readyState < 3)
            updateStatus('buffering');
        });
        createdArt.on('video:pause', () => updateStatus('paused'));
        createdArt.on('video:ended', () => updateStatus('ended'));

        const notifyMpegtsProgress = () => {
          mpegtsController?.notifyMediaProgress();
        };
        createdArt.on('video:seeked', notifyMpegtsProgress);
        createdArt.on('video:canplay', notifyMpegtsProgress);
        createdArt.on('video:playing', notifyMpegtsProgress);
        createdArt.on('video:timeupdate', notifyMpegtsProgress);
      } catch (initializationError) {
        destroySession();
        if (disposed) return;
        setError(
          initializationError instanceof PlaybackConfigurationError
            ? initializationError.reason
            : 'unknown',
        );
        setStatus('ready');
      }
    };

    void initialize();
    return () => {
      disposed = true;
      destroySession();
    };
  }, [
    playUrl,
    resolvedMediaType,
    isLive,
    mediaDurationSecs,
    mediaFileSizeBytes,
    reloadKey,
    connection,
  ]);

  const playbackError = configurationError ?? resolutionError ?? error;
  const playbackStatus = playbackError
    ? 'error'
    : resolving
      ? 'resolving'
      : status;
  const loading = ['resolving', 'connecting', 'buffering'].includes(
    playbackStatus,
  );
  return {
    containerRef,
    error: playbackError,
    loading,
    reload,
    status: playbackStatus,
    connection,
  };
}
