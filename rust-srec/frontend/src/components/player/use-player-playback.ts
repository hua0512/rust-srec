import { useCallback, useEffect, useRef, useState } from 'react';
import { toast } from 'sonner';
import { resolvePlayerMediaType, type PlayerMediaType } from '@/lib/media';
import { resolveUrl } from '@/server/functions/parse';
import { isDesktopBuild } from '@/utils/desktop';
import { BASE_URL } from '@/utils/env';
import { getDesktopAccessToken } from '@/utils/session';
import { MpegtsPlaybackController } from './mpegts-playback';

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
  source: PlaybackSource;
}

interface UseResolvedSourceOptions {
  url: string;
  headers?: Record<string, string>;
  title?: string;
  streamData?: unknown;
  reloadKey: number;
}

export interface UsePlayerPlaybackOptions {
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
}: BuildPlaybackUrlOptions): string {
  if (!headers || Object.keys(headers).length === 0) return url;

  const query = `url=${encodeURIComponent(url)}&headers=${encodeURIComponent(JSON.stringify(headers))}`;
  if (!desktopBuild) return `/stream-proxy?${query}`;
  if (!desktopToken) return url;

  return `${baseUrl.replace(/\/$/, '')}/stream-proxy?${query}&token=${encodeURIComponent(desktopToken)}`;
}

function matchesRequest(
  resolved: ResolvedSource | null,
  options: UseResolvedSourceOptions,
): boolean {
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
      let sourceUrl = url;
      try {
        const response = await resolveUrl({
          data: {
            url: title,
            stream_info: streamData,
          },
        });
        if (disposed) return;

        if (response.success && response.stream_info) {
          sourceUrl = response.stream_info.url || url;
        } else {
          console.warn(
            '[PlayerCard] URL resolution failed, using original:',
            response.error,
          );
          toast.error(
            `Resolution failed: ${response.error || 'Unknown error'}`,
          );
        }
      } catch (error) {
        if (disposed) return;
        console.error('[PlayerCard] Resolution error:', error);
      }

      if (!disposed) {
        setResolved({ request, source: { url: sourceUrl, headers } });
      }
    };

    void resolve();
    return () => {
      disposed = true;
    };
  }, [url, headers, title, streamData, reloadKey]);

  if (!needsResolution) {
    return { source: { url, headers }, resolving: false };
  }
  if (!matchesRequest(resolved, options)) {
    return { source: null, resolving: true };
  }
  return { source: resolved.source, resolving: false };
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
  } = options;
  const containerRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<ArtplayerInstance | null>(null);
  const volumeRef = useRef(volume);
  const mutedRef = useRef(muted);
  const defaultWebFullscreenRef = useRef(defaultWebFullscreen);
  const onVolumeChangeRef = useRef(onVolumeChange);
  const onMuteChangeRef = useRef(onMuteChange);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);

  volumeRef.current = volume;
  mutedRef.current = muted;
  defaultWebFullscreenRef.current = defaultWebFullscreen;
  onVolumeChangeRef.current = onVolumeChange;
  onMuteChangeRef.current = onMuteChange;

  const { source, resolving } = useResolvedSource({
    url,
    headers,
    title,
    streamData,
    reloadKey,
  });
  const desktopBuild = isDesktopBuild();
  const desktopToken = desktopBuild ? getDesktopAccessToken() : null;
  const playUrl = source
    ? buildPlaybackUrl({
        ...source,
        desktopBuild,
        desktopToken,
        baseUrl: BASE_URL,
      })
    : null;
  const resolvedMediaType = source
    ? resolvePlayerMediaType(mediaType, source.url, title)
    : null;

  const reload = useCallback(() => {
    setError(null);
    setLoading(true);
    setReloadKey((current) => current + 1);
  }, []);

  useEffect(() => {
    if (!resolving) return;
    setError(null);
    setLoading(true);
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
      setLoading(true);

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
                    setError(`HLS Error: ${data.type} - ${data.details}`);
                    setLoading(false);
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
            throw new Error(
              'MPEG-TS playback is not supported by this browser',
            );
          }

          mpegtsController = new MpegtsPlaybackController(mpegts, {
            mediaType: mpegtsType,
            isLive,
            durationSecs: mediaDurationSecs,
            fileSizeBytes: mediaFileSizeBytes,
            onLoadingChange: (nextLoading) => {
              if (!disposed) setLoading(nextLoading);
            },
            onError: ({ type, details, data }) => {
              console.error('MPEG-TS Error:', { type, details, data });
              if (!disposed) {
                setError(`MPEG-TS Error: ${type} - ${details}`);
              }
            },
            onStalled: () => {
              if (!disposed) {
                setError('Playback stalled while seeking. Reload the player.');
              }
            },
            onWarning: (message, warning) => {
              console.warn(`${message}:`, warning);
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
          setLoading(false);
          setError(null);
        });
        createdArt.on('error', (playerError: Error) => {
          if (disposed) return;
          mpegtsController?.cancelSeekRecovery();
          console.error('Player Error:', playerError);
          setError(`Player Error: ${playerError.message || 'Unknown error'}`);
          setLoading(false);
        });
        createdArt.on('video:volumechange', () => {
          onVolumeChangeRef.current?.(createdArt.volume);
          onMuteChangeRef.current?.(createdArt.muted);
        });
        createdArt.on('seek', (currentTime) => {
          if (!mpegtsController?.seek(currentTime)) return;
          setError(null);
        });

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
          `Failed to initialize player: ${initializationError instanceof Error ? initializationError.message : 'Unknown error'}`,
        );
        setLoading(false);
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
  ]);

  return { containerRef, error, loading, reload };
}
