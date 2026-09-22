import { useEffect, useId, useState, type ReactNode } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import {
  AlertCircle,
  Info,
  Loader2,
  RefreshCcw,
  Settings2,
  X,
} from 'lucide-react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';
import { usePlayerPlayback } from './use-player-playback';
import {
  playbackPresetMessages,
  type PlaybackPreset,
} from './playback-presets';
import { PlaybackDetails, type SourceMediaDetails } from './playback-details';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  playbackErrorMessages,
  playbackStatusMessages,
  type ConnectionMode,
} from './playback-state';

export interface PlayerCardProps {
  url: string;
  title?: string;
  sourceUrl?: string;
  creator?: string;
  quality?: string;
  sourceDetails?: SourceMediaDetails;
  onRefreshSource?: () => Promise<void>;
  headers?: Record<string, string>;
  streamData?: unknown;
  onRemove?: () => void;
  className?: string;
  contentClassName?: string;
  settingsContent?: ReactNode;
  muted?: boolean;
  volume?: number;
  onVolumeChange?: (volume: number) => void;
  onMuteChange?: (muted: boolean) => void;
  defaultWebFullscreen?: boolean;
  mediaType?: string;
  isLive?: boolean;
  mediaDurationSecs?: number | null;
  mediaFileSizeBytes?: number;
}

export function PlayerCard({
  url,
  title,
  sourceUrl,
  creator,
  quality,
  sourceDetails,
  onRefreshSource,
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
  mediaType,
  isLive = false,
  mediaDurationSecs,
  mediaFileSizeBytes,
}: PlayerCardProps) {
  const { i18n } = useLingui();
  const [connectionMode, setConnectionMode] = useState<ConnectionMode>('auto');
  const [playbackPreset, setPlaybackPreset] =
    useState<PlaybackPreset>('balanced');
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshFailed, setRefreshFailed] = useState(false);
  const connectionId = useId();
  const presetId = useId();
  const {
    containerRef,
    error: playbackError,
    loading: playbackLoading,
    reload,
    status: playbackStatus,
    connection,
    statistics,
    supportsLivePresets,
  } = usePlayerPlayback({
    playbackPreset,
    detailsEnabled: detailsOpen,
    url,
    headers,
    title,
    sourceUrl,
    connectionMode,
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
  });

  useEffect(() => {
    setRefreshFailed(false);
  }, [url, streamData]);
  const error = refreshFailed ? 'resolution' : playbackError;
  const status = refreshing ? 'resolving' : error ? 'error' : playbackStatus;
  const loading = refreshing || playbackLoading;
  const hasHeaders = Object.keys(headers ?? {}).length > 0;
  const retry = () => {
    setRefreshFailed(false);
    reload();
  };
  const refreshSource = async () => {
    if (!onRefreshSource || refreshing) return;
    setRefreshing(true);
    setRefreshFailed(false);
    try {
      await onRefreshSource();
    } catch {
      setRefreshFailed(true);
    } finally {
      setRefreshing(false);
    }
  };
  let sourceLabel = '';
  if (sourceUrl) {
    try {
      const source = new URL(sourceUrl);
      sourceLabel = `${source.host}${source.pathname}`;
    } catch {
      sourceLabel = i18n._(msg`Stream source`);
    }
  }

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

      <CardHeader className="relative flex flex-row flex-wrap items-start justify-between gap-3 pb-2 space-y-0 z-10">
        <div className="flex items-start gap-3 min-w-0 flex-1">
          <div className="p-2 rounded-xl bg-primary/10 ring-1 ring-inset ring-primary/20 transition-transform duration-500 group-hover:scale-110">
            <div
              className={cn(
                'h-4 w-4 rounded-full',
                status === 'playing'
                  ? 'bg-emerald-500'
                  : error
                    ? 'bg-destructive'
                    : 'bg-muted-foreground',
              )}
            />
          </div>
          <div className="flex flex-col min-w-0">
            <CardTitle className="text-sm font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
              {title || i18n._(msg`Video Player`)}
            </CardTitle>
            {(creator || sourceLabel) && (
              <p
                className="text-xs text-muted-foreground truncate"
                title={sourceLabel}
              >
                {creator ? `${creator} · ${sourceLabel}` : sourceLabel}
              </p>
            )}
            <div className="flex flex-wrap items-center gap-2 mt-2 text-xs">
              <Badge
                variant={error ? 'destructive' : 'secondary'}
                className="gap-1"
                role="status"
                aria-live="polite"
              >
                {loading && !error && (
                  <Loader2 className="h-3 w-3 animate-spin motion-reduce:animate-none" />
                )}
                {i18n._(playbackStatusMessages[status])}
              </Badge>
              {isLive && (
                <Badge variant="outline">
                  <Trans>LIVE</Trans>
                </Badge>
              )}
              {quality && (
                <span className="text-muted-foreground">{quality}</span>
              )}
              {sourceUrl && (
                <span className="text-muted-foreground">
                  {connectionMode === 'auto' && (
                    <>
                      <Trans>Auto</Trans>
                      {' · '}
                    </>
                  )}
                  {connection === 'proxy' ? (
                    <Trans>Server proxy</Trans>
                  ) : (
                    <Trans>Direct</Trans>
                  )}
                </span>
              )}
            </div>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
            onClick={retry}
            disabled={refreshing}
            aria-label={i18n._(msg`Reload Player`)}
            title={i18n._(msg`Reload Player`)}
          >
            <RefreshCcw className="h-4 w-4" />
          </Button>
          <Popover open={detailsOpen} onOpenChange={setDetailsOpen}>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 rounded-full text-muted-foreground"
                aria-label={i18n._(msg`Playback details`)}
              >
                <Info className="h-4 w-4" />
              </Button>
            </PopoverTrigger>
            <PopoverContent
              align="end"
              className="w-[320px] max-w-[calc(100vw-2rem)] max-h-[70vh] overflow-y-auto z-[200] motion-reduce:animate-none"
            >
              <PlaybackDetails
                statistics={statistics}
                source={sourceDetails}
                isLive={isLive}
              />
            </PopoverContent>
          </Popover>
          {(settingsContent || sourceUrl || isLive) && (
            <Popover open={settingsOpen} onOpenChange={setSettingsOpen}>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
                  aria-label={i18n._(msg`Player settings`)}
                >
                  <Settings2 className="h-4 w-4" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="end"
                className="w-[320px] max-w-[calc(100vw-2rem)] max-h-[70vh] overflow-y-auto p-4 backdrop-blur-xl bg-background/95 border-border/40 text-foreground z-[200] motion-reduce:animate-none"
              >
                {sourceUrl && (
                  <div className="space-y-3 mb-4 pb-4 border-b border-border/40">
                    <label
                      htmlFor={connectionId}
                      className="text-sm font-medium"
                    >
                      <Trans>Connection</Trans>
                    </label>
                    <Select
                      value={connectionMode}
                      onValueChange={(value) => {
                        setRefreshFailed(false);
                        setConnectionMode(value as ConnectionMode);
                      }}
                      disabled={refreshing}
                    >
                      <SelectTrigger
                        id={connectionId}
                        aria-describedby={`${connectionId}-help`}
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent className="z-[300]">
                        <SelectItem value="auto">
                          <Trans>Auto</Trans>
                        </SelectItem>
                        <SelectItem value="direct" disabled={hasHeaders}>
                          <Trans>Direct</Trans>
                        </SelectItem>
                        <SelectItem value="proxy">
                          <Trans>Server proxy</Trans>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    <p
                      id={`${connectionId}-help`}
                      className="text-xs text-muted-foreground"
                    >
                      {hasHeaders ? (
                        <Trans>
                          This source needs request headers, so Auto uses the
                          server proxy. Direct is unavailable.
                        </Trans>
                      ) : (
                        <Trans>
                          Auto connects directly. Choose Server proxy if the
                          browser cannot load the source.
                        </Trans>
                      )}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      <Trans>
                        Server proxy relays playback through this app's server.
                        It does not select an upstream network proxy.
                      </Trans>
                    </p>
                    {onRefreshSource && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="w-full"
                        disabled={refreshing}
                        onClick={() => void refreshSource()}
                      >
                        <Trans>Refresh stream URL</Trans>
                      </Button>
                    )}
                  </div>
                )}
                {isLive && (
                  <div className="space-y-3 mb-4 pb-4 border-b border-border/40">
                    <label htmlFor={presetId} className="text-sm font-medium">
                      <Trans>Playback preference</Trans>
                    </label>
                    <Select
                      value={playbackPreset}
                      onValueChange={(value) =>
                        setPlaybackPreset(value as PlaybackPreset)
                      }
                      disabled={!supportsLivePresets || refreshing}
                    >
                      <SelectTrigger
                        id={presetId}
                        aria-describedby={`${presetId}-help`}
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent className="z-[300]">
                        <SelectItem value="low-latency">
                          <Trans>Low latency</Trans>
                        </SelectItem>
                        <SelectItem value="balanced">
                          <Trans>Balanced</Trans>
                        </SelectItem>
                        <SelectItem value="smooth">
                          <Trans>Smooth playback</Trans>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    <p
                      id={`${presetId}-help`}
                      className="text-xs text-muted-foreground"
                    >
                      {supportsLivePresets ? (
                        i18n._(playbackPresetMessages[playbackPreset])
                      ) : (
                        <Trans>
                          Buffering presets are unavailable for this playback
                          path.
                        </Trans>
                      )}
                    </p>
                    {supportsLivePresets && (
                      <p className="text-xs text-muted-foreground">
                        <Trans>
                          Changing this preference restarts this live player.
                          The source and network still determine the actual
                          delay.
                        </Trans>
                      </p>
                    )}
                  </div>
                )}
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
              aria-label={i18n._(msg`Remove player`)}
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
        <div ref={containerRef} className="w-full h-full absolute inset-0" />
        {loading && (!error || refreshing) && (
          <div className="pointer-events-none absolute inset-0 z-[5] flex items-center justify-center bg-black/20">
            <Loader2 className="h-8 w-8 animate-spin motion-reduce:animate-none text-white/80 drop-shadow" />
          </div>
        )}
        {error && !refreshing && (
          <div
            role="alert"
            className="absolute inset-0 z-10 flex flex-col items-center justify-center h-full text-center space-y-3 p-6 bg-black/90"
          >
            <div className="p-3 rounded-full bg-destructive/10 text-destructive mb-2">
              <AlertCircle className="h-8 w-8" />
            </div>
            <p className="text-sm font-medium text-destructive">
              <Trans>Playback Error</Trans>
            </p>
            <p className="text-xs text-muted-foreground max-w-[250px]">
              {i18n._(playbackErrorMessages[error])}
            </p>
            <div className="flex flex-wrap justify-center gap-2">
              <Button size="sm" onClick={retry}>
                <Trans>Retry</Trans>
              </Button>
              {onRefreshSource && (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void refreshSource()}
                >
                  <Trans>Refresh stream URL</Trans>
                </Button>
              )}
              {sourceUrl && connection === 'direct' && (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    setRefreshFailed(false);
                    setConnectionMode('proxy');
                  }}
                >
                  <Trans>Try server proxy</Trans>
                </Button>
              )}
              {settingsContent && (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setSettingsOpen(true)}
                >
                  <Trans>Change source</Trans>
                </Button>
              )}
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
