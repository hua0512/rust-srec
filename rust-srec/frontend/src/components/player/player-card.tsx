import { useEffect, useRef, useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { AlertCircle, Loader2, RefreshCcw, Settings2, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { resolveUrl } from '@/server/functions/parse';
import { toast } from 'sonner';

import { BASE_URL } from '@/utils/env';
import { getDesktopAccessToken } from '@/utils/session';
import { isDesktopBuild } from '@/utils/desktop';

export interface PlayerCardProps {
  url: string;
  title?: string;
  headers?: Record<string, string>;
  streamData?: any; // The original stream object for resolution
  onRemove?: () => void;
  className?: string;
  contentClassName?: string;
  settingsContent?: React.ReactNode;
  muted?: boolean;
  volume?: number;
  onVolumeChange?: (volume: number) => void;
  onMuteChange?: (muted: boolean) => void;
  defaultWebFullscreen?: boolean;
}

export function PlayerCard({
  url,
  title,
  headers,
  streamData,
  onRemove,
  className,
  settingsContent,
  contentClassName,
  muted = false,
  volume = 0.5,
  onVolumeChange,
  onMuteChange,
  defaultWebFullscreen = false,
}: PlayerCardProps) {
  const artRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<any>(null); // Type loose for dynamic import
  const hlsRef = useRef<any>(null);
  const flvRef = useRef<any>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [resolving, setResolving] = useState(false);
  const [currentUrl, setCurrentUrl] = useState(url);
  const [currentHeaders, setCurrentHeaders] = useState(headers);
  const [resolvedStream, setResolvedStream] = useState<any>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const handleRefresh = () => {
    setRefreshKey((prev) => prev + 1);
    setLoading(true);
  };

  // Auto-resolve URL on mount or refresh if streamData is present
  useEffect(() => {
    let disposed = false;
    const resolve = async () => {
      if (!streamData || !title) {
        setCurrentUrl(url);
        setCurrentHeaders(headers);
        setResolvedStream(null);
        return;
      }

      // Use the previously resolved stream info as base ONLY if it matches the current selection
      // This prevents using resolved info from one quality for another
      const baseStreamInfo =
        resolvedStream &&
        resolvedStream.quality === streamData?.quality &&
        resolvedStream.cdn === streamData?.cdn
          ? resolvedStream
          : streamData;

      setResolving(true);
      try {
        const response = await resolveUrl({
          data: {
            url: title,
            stream_info: baseStreamInfo,
          },
        });
        if (disposed) return;
        console.log('[PlayerCard] Headers:', headers);
        console.log('[PlayerCard] Resolve Response:', response);

        if (response.success && response.stream_info) {
          console.log('[PlayerCard] Resolved URL:', response.stream_info.url);
          setCurrentUrl(response.stream_info.url || url);
          // Ensure use headers from resolved stream info if available
          // setCurrentHeaders(response.stream_info.headers || headers);
          setResolvedStream(response.stream_info);
        } else {
          console.warn(
            '[PlayerCard] URL resolution failed, using original:',
            response.error,
          );
          toast.error(
            `Resolution failed: ${response.error || 'Unknown error'}`,
          );
          setCurrentUrl(url);
          setCurrentHeaders(headers);
        }
      } catch (err) {
        console.error('[PlayerCard] Resolution error:', err);
        if (disposed) return;
        setCurrentUrl(url);
        setCurrentHeaders(headers);
      } finally {
        if (!disposed) setResolving(false);
      }
    };

    void resolve();

    return () => {
      disposed = true;
    };
  }, [url, headers, streamData, title, refreshKey]);

  useEffect(() => {
    if (!artRef.current || resolving) return;

    let disposed = false;

    if (playerRef.current) {
      const video: HTMLMediaElement | undefined = playerRef.current?.video;
      if (video) {
        try {
          video.pause();
          video.removeAttribute('src');
          video.load();
        } catch {
          // ignore
        }
      }
      playerRef.current.destroy(false);
      playerRef.current = null;
    }

    // Clear only if we are about to create a new one, ensuring clean slate
    if (artRef.current) {
      artRef.current.innerHTML = '';
    }

    const checkString = (currentUrl + (title || '')).toLowerCase();
    const isHLS = checkString.includes('.m3u8') || checkString.includes('m3u8');
    const isMPEGTS =
      checkString.includes('.flv') || checkString.includes('.ts');
    const isMP4 = checkString.includes('.mp4');
    const isMKV = checkString.includes('.mkv');
    const isAudio =
      checkString.includes('.mp3') ||
      checkString.includes('.wav') ||
      checkString.includes('.ogg');

    // Build proxy URL if headers are needed
    const shouldProxy =
      !!currentHeaders && Object.keys(currentHeaders).length > 0;

    const desktopBuild = isDesktopBuild();
    const desktopToken = desktopBuild ? getDesktopAccessToken() : null;
    const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;

    const playUrl = shouldProxy
      ? desktopBuild
        ? desktopToken
          ? `${baseUrl}/stream-proxy?url=${encodeURIComponent(currentUrl)}&headers=${encodeURIComponent(JSON.stringify(currentHeaders))}&token=${encodeURIComponent(desktopToken)}`
          : currentUrl
        : `/stream-proxy?url=${encodeURIComponent(currentUrl)}&headers=${encodeURIComponent(JSON.stringify(currentHeaders))}`
      : currentUrl;

    console.log('[PlayerCard] Init:', {
      originalUrl: url,
      currentUrl,
      headers: currentHeaders,
      shouldProxy,
      playUrl,
      isHLS,
      isMPEGTS,
      isMP4,
      isMKV,
      isAudio,
    });

    const initPlayer = async () => {
      try {
        const { default: Artplayer } = await import('artplayer');
        if (disposed || !artRef.current) return;

        const options: any = {
          container: artRef.current,
          url: playUrl,
          autoplay: true,
          volume: volume,
          muted: muted,
          autoSize: false,
          pip: true,
          mutex: false,
          setting: true,
          playbackRate: true,
          aspectRatio: true,
          fullscreen: true,
          fullscreenWeb: true,
          miniProgressBar: true,
          theme: '#3b82f6',
          type: isHLS
            ? 'm3u8'
            : isMPEGTS
              ? 'flv'
              : isMP4
                ? 'mp4'
                : isMKV
                  ? 'mkv'
                  : isAudio
                    ? 'mp3'
                    : 'auto',
        };

        // Custom type for HLS
        if (isHLS) {
          try {
            // Dynamically import Hls.js
            const { default: Hls } = await import('hls.js');
            if (Hls.isSupported()) {
              console.log('[PlayerCard] Using custom type for HLS');
              options.customType = {
                m3u8: (video: HTMLMediaElement, url: string) => {
                  const hls = new Hls({
                    enableWorker: true,
                    lowLatencyMode: true,
                  });
                  hlsRef.current = hls;
                  hls.loadSource(url);
                  hls.attachMedia(video);
                  hls.on(Hls.Events.ERROR, (_event: any, data: any) => {
                    if (data.fatal) {
                      setError(`HLS Error: ${data.type} - ${data.details}`);
                      setLoading(false);
                    }
                  });
                },
              };
            }
          } catch (e) {
            console.error('Failed to load hls.js', e);
          }
        }

        // Custom type for MPEG-TS
        if (isMPEGTS) {
          try {
            // Dynamically import mpegts.js
            const { default: mpegts } = await import('mpegts.js');
            if (mpegts.isSupported()) {
              console.log('[PlayerCard] Using custom type for MPEG-TS');
              options.customType = {
                flv: (video: HTMLMediaElement, url: string) => {
                  const player = mpegts.createPlayer({
                    type: 'flv',
                    url: url,
                    isLive: true,
                    cors: true,
                  });
                  flvRef.current = player;
                  player.attachMediaElement(video);
                  player.load();
                  player.on(
                    mpegts.Events.ERROR,
                    (type: any, details: any, data: any) => {
                      console.error('MPEG-TS Error:', { type, details, data });
                      setError(`MPEG-TS Error: ${type} - ${details}`);
                      setLoading(false);
                    },
                  );
                },
              };
            }
          } catch (e) {
            console.error('Failed to load mpegts.js', e);
          }
        }

        const art = new Artplayer(options);
        if (disposed) {
          art.destroy(false);
          return;
        }
        playerRef.current = art;

        art.on('ready', () => {
          if (defaultWebFullscreen) {
            art.fullscreenWeb = true;
          }
          setLoading(false);
          setError(null);
        });

        art.on('error', (err: any) => {
          console.log('Player Error: ', err);
          setError(`Player Error: ${err?.message || 'Unknown error'}`);
          setLoading(false);
        });

        art.on('video:volumechange', () => {
          onVolumeChange?.(art.volume);
          onMuteChange?.(art.muted);
        });
      } catch (err) {
        setError(
          `Failed to initialize player: ${err instanceof Error ? err.message : 'Unknown error'}`,
        );
        setLoading(false);
      }
    };

    void initPlayer();

    return () => {
      disposed = true;
      if (hlsRef.current) {
        const hls = hlsRef.current;
        hls.stopLoad?.();
        hls.detachMedia?.();
        hls.destroy?.();
        hlsRef.current = null;
      }
      if (flvRef.current) {
        const flv = flvRef.current;
        flv.unload?.();
        flv.detachMediaElement?.();
        flv.destroy?.();
        flvRef.current = null;
      }
      if (playerRef.current) {
        const video: HTMLMediaElement | undefined = playerRef.current?.video;
        if (video) {
          try {
            video.pause();
            video.removeAttribute('src');
            video.load();
          } catch {
            // ignore
          }
        }
        playerRef.current.destroy(false);
        playerRef.current = null;
      }
      // Aggressive cleanup for strict mode
      if (artRef.current) {
        artRef.current.innerHTML = '';
      }
    };
  }, [
    currentUrl,
    currentHeaders,
    resolving,
    title,
    refreshKey,
    defaultWebFullscreen,
  ]);

  return (
    <Card
      className={cn(
        'relative h-full flex flex-col transition-all duration-500 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20',
        className,
      )}
    >
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center justify-between gap-4 pb-2 space-y-0 z-10">
        <div className="flex items-center gap-3 min-w-0">
          <div className="p-2 rounded-xl bg-primary/10 ring-1 ring-inset ring-primary/20 transition-transform duration-500 group-hover:scale-110">
            <div className="h-4 w-4 bg-primary rounded-full animate-pulse" />
          </div>
          <div className="flex flex-col min-w-0">
            <CardTitle className="text-sm font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
              {title || 'Video Player'}
            </CardTitle>
          </div>
        </div>

        <div className="flex items-center gap-2">
          {loading && (
            <Badge
              variant="secondary"
              className="gap-1 bg-background/50 backdrop-blur"
            >
              <Loader2 className="h-3 w-3 animate-spin" />
              Loading
            </Badge>
          )}
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
            onClick={handleRefresh}
            title="Reload Player"
          >
            <RefreshCcw className="h-4 w-4" />
          </Button>
          {settingsContent && (
            <Popover>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
                >
                  <Settings2 className="h-4 w-4" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="end"
                className="w-[320px] p-4 backdrop-blur-xl bg-background/80 border-border/40 text-foreground z-[200]"
              >
                {settingsContent}
              </PopoverContent>
            </Popover>
          )}
          {onRemove && (
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 text-muted-foreground/40 hover:text-destructive hover:bg-destructive/10 transition-colors rounded-full"
              onClick={onRemove}
            >
              <X className="h-4 w-4" />
            </Button>
          )}
        </div>
      </CardHeader>

      <CardContent
        className={cn(
          'relative p-0 flex-1 min-h-[500px] bg-black/50 group-hover:bg-black/40 transition-colors rounded-b-xl overflow-hidden',
          contentClassName,
        )}
      >
        {error ? (
          <div className="flex flex-col items-center justify-center h-full text-center space-y-3 p-8">
            <div className="p-3 rounded-full bg-destructive/10 text-destructive mb-2">
              <AlertCircle className="h-8 w-8" />
            </div>
            <p className="text-sm font-medium text-destructive">
              Playback Error
            </p>
            <p className="text-xs text-muted-foreground max-w-[250px]">
              {error}
            </p>
          </div>
        ) : (
          <div ref={artRef} className="w-full h-full absolute inset-0" />
        )}
      </CardContent>
    </Card>
  );
}
