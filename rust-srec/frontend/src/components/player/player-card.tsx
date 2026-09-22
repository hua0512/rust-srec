import { useEffect, useId, useState, type ReactNode } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Activity,
  AlertCircle,
  Gauge,
  Loader2,
  Network,
  RefreshCcw,
  Rabbit,
  Scale,
  Server,
  Settings2,
  SlidersHorizontal,
  Sparkles,
  Tv,
  Turtle,
  X,
  Zap,
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
import { OptionGroup, SettingsSection } from './option-group';
import {
  playbackErrorMessages,
  playbackStatusMessages,
  type ConnectionMode,
  type PlaybackStatus,
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

type SettingsTab = 'stream' | 'playback';

// Steady states are already visible in the video itself, so only transitional
// and failure states earn a visible chip; the rest stay screen-reader only.
const noticeableStatuses = new Set<PlaybackStatus>([
  'resolving',
  'connecting',
  'buffering',
  'ended',
  'error',
]);

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
  const [settingsTab, setSettingsTab] = useState<SettingsTab>(
    settingsContent ? 'stream' : 'playback',
  );
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
  const hasPlaybackSettings = Boolean(sourceUrl) || isLive;
  const hasSettings = Boolean(settingsContent) || hasPlaybackSettings;
  const showTabs = Boolean(settingsContent) && hasPlaybackSettings;
  const activeTab: SettingsTab = !settingsContent
    ? 'playback'
    : !hasPlaybackSettings
      ? 'stream'
      : settingsTab;
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
  const chooseConnection = (mode: ConnectionMode) => {
    setRefreshFailed(false);
    setConnectionMode(mode);
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
  const subtitle = [creator, sourceLabel].filter(Boolean).join(' · ');

  const playbackSettings = (
    <div className="space-y-5">
      {sourceUrl && (
        <SettingsSection
          id={connectionId}
          icon={Network}
          title={<Trans>Connection</Trans>}
          footer={
            hasHeaders ? (
              <Trans>
                This source needs request headers, so Direct is unavailable.
              </Trans>
            ) : (
              <Trans>
                Server proxy relays playback through this app's server with the
                source's upstream proxy settings.
              </Trans>
            )
          }
        >
          <OptionGroup
            aria-labelledby={connectionId}
            aria-describedby={`${connectionId}-help`}
            value={connectionMode}
            onValueChange={chooseConnection}
            disabled={refreshing}
            options={[
              { value: 'auto', label: i18n._(msg`Auto`), icon: Sparkles },
              {
                value: 'direct',
                label: i18n._(msg`Direct`),
                icon: Zap,
                disabled: hasHeaders,
              },
              {
                value: 'proxy',
                label: i18n._(msg`Server proxy`),
                icon: Server,
              },
            ]}
          />
        </SettingsSection>
      )}
      {isLive && (
        <SettingsSection
          id={presetId}
          icon={Gauge}
          title={<Trans>Latency</Trans>}
          footer={
            supportsLivePresets ? (
              <>
                {i18n._(playbackPresetMessages[playbackPreset])}{' '}
                <Trans>Changing this restarts the player.</Trans>
              </>
            ) : (
              <Trans>
                Buffering presets are unavailable for this playback path.
              </Trans>
            )
          }
        >
          <OptionGroup
            aria-labelledby={presetId}
            aria-describedby={`${presetId}-help`}
            value={playbackPreset}
            onValueChange={setPlaybackPreset}
            disabled={!supportsLivePresets || refreshing}
            options={[
              { value: 'low-latency', label: i18n._(msg`Low`), icon: Rabbit },
              { value: 'balanced', label: i18n._(msg`Balanced`), icon: Scale },
              { value: 'smooth', label: i18n._(msg`Smooth`), icon: Turtle },
            ]}
          />
        </SettingsSection>
      )}
      {onRefreshSource && (
        <Button
          variant="outline"
          size="sm"
          className="w-full gap-2"
          disabled={refreshing}
          onClick={() => void refreshSource()}
        >
          <RefreshCcw
            className={cn(
              'h-3.5 w-3.5',
              refreshing && 'animate-spin motion-reduce:animate-none',
            )}
          />
          <Trans>Refresh stream URL</Trans>
        </Button>
      )}
    </div>
  );

  return (
    <Card
      className={cn(
        'relative h-full gap-0 overflow-hidden border-border/40 bg-card/60 py-0 backdrop-blur-xl',
        className,
      )}
    >
      <CardHeader className="flex flex-row items-center gap-3 space-y-0 px-4 py-2.5">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
            <CardTitle className="min-w-0 max-w-full truncate text-sm font-semibold tracking-tight">
              {title || i18n._(msg`Video Player`)}
            </CardTitle>
            {isLive && (
              <span className="inline-flex shrink-0 items-center gap-1 rounded bg-red-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-red-500">
                <span className="h-1.5 w-1.5 rounded-full bg-red-500 animate-pulse motion-reduce:animate-none" />
                <Trans>LIVE</Trans>
              </span>
            )}
            {quality && (
              <span className="shrink-0 rounded border border-border/60 px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
                {quality}
              </span>
            )}
            <span
              role="status"
              aria-live="polite"
              className={cn(
                noticeableStatuses.has(status)
                  ? cn(
                      'inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium',
                      error
                        ? 'bg-destructive/15 text-destructive'
                        : 'bg-muted text-muted-foreground',
                    )
                  : 'sr-only',
              )}
            >
              {loading && !error && (
                <Loader2 className="h-3 w-3 animate-spin motion-reduce:animate-none" />
              )}
              {i18n._(playbackStatusMessages[status])}
            </span>
          </div>
          {(subtitle || (sourceUrl && connection === 'proxy')) && (
            <p
              className="mt-0.5 truncate text-xs text-muted-foreground"
              title={subtitle}
            >
              {subtitle}
              {sourceUrl && connection === 'proxy' && (
                <>
                  {subtitle && ' · '}
                  <Trans>via server proxy</Trans>
                </>
              )}
            </p>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-0.5">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 text-muted-foreground hover:text-foreground"
            onClick={retry}
            disabled={refreshing}
            aria-label={i18n._(msg`Reload Player`)}
            title={i18n._(msg`Reload Player`)}
          >
            <RefreshCcw className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className={cn(
              'h-8 w-8 text-muted-foreground hover:text-foreground',
              detailsOpen && 'bg-accent text-foreground',
            )}
            onClick={() => setDetailsOpen((open) => !open)}
            aria-pressed={detailsOpen}
            aria-label={i18n._(msg`Playback details`)}
            title={i18n._(msg`Playback details`)}
          >
            <Activity className="h-4 w-4" />
          </Button>
          {hasSettings && (
            <Popover open={settingsOpen} onOpenChange={setSettingsOpen}>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 text-muted-foreground hover:text-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground"
                  aria-label={i18n._(msg`Player settings`)}
                  title={i18n._(msg`Player settings`)}
                >
                  <Settings2 className="h-4 w-4" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="end"
                className="z-[200] max-h-[70vh] w-[340px] max-w-[calc(100vw-2rem)] overflow-y-auto rounded-xl p-3 shadow-xl motion-reduce:animate-none"
              >
                {showTabs ? (
                  <Tabs
                    value={activeTab}
                    onValueChange={(value) =>
                      setSettingsTab(value as SettingsTab)
                    }
                    className="gap-4"
                  >
                    <TabsList className="h-9 w-full">
                      <TabsTrigger value="stream" className="gap-1.5 text-xs">
                        <Tv className="h-3.5 w-3.5" />
                        <Trans>Stream</Trans>
                      </TabsTrigger>
                      <TabsTrigger value="playback" className="gap-1.5 text-xs">
                        <SlidersHorizontal className="h-3.5 w-3.5" />
                        <Trans>Playback</Trans>
                      </TabsTrigger>
                    </TabsList>
                    <TabsContent value="stream">{settingsContent}</TabsContent>
                    <TabsContent value="playback">
                      {playbackSettings}
                    </TabsContent>
                  </Tabs>
                ) : activeTab === 'stream' ? (
                  settingsContent
                ) : (
                  playbackSettings
                )}
              </PopoverContent>
            </Popover>
          )}
          {onRemove && (
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
              onClick={onRemove}
              aria-label={i18n._(msg`Remove player`)}
              title={i18n._(msg`Remove player`)}
            >
              <X className="h-4 w-4" />
            </Button>
          )}
        </div>
      </CardHeader>

      <CardContent
        className={cn(
          'relative min-h-[500px] flex-1 overflow-hidden bg-black p-0',
          contentClassName,
        )}
      >
        <div ref={containerRef} className="absolute inset-0 h-full w-full" />
        {detailsOpen && !error && (
          <div
            role="region"
            aria-label={i18n._(msg`Playback details`)}
            className="absolute left-3 top-3 z-[6] w-60 max-w-[calc(100%-1.5rem)] rounded-lg bg-black/75 p-3 text-white shadow-lg ring-1 ring-white/10 backdrop-blur-md"
          >
            <button
              type="button"
              className="absolute right-1.5 top-1.5 rounded p-1 text-white/60 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/60"
              onClick={() => setDetailsOpen(false)}
              aria-label={i18n._(msg`Close playback details`)}
            >
              <X className="h-3.5 w-3.5" />
            </button>
            <PlaybackDetails
              statistics={statistics}
              source={sourceDetails}
              isLive={isLive}
              liveEdgeHint={i18n._(
                msg`How far playback trails the available stream, not the total broadcast delay.`,
              )}
            />
          </div>
        )}
        {loading && (!error || refreshing) && (
          <div className="pointer-events-none absolute inset-0 z-[5] flex items-center justify-center bg-black/20">
            <Loader2 className="h-8 w-8 animate-spin text-white/80 drop-shadow motion-reduce:animate-none" />
          </div>
        )}
        {error && !refreshing && (
          <div
            role="alert"
            className="absolute inset-0 z-10 flex h-full flex-col items-center justify-center gap-3 bg-black/90 p-6 text-center"
          >
            <AlertCircle className="h-8 w-8 text-destructive" />
            <div className="space-y-1">
              <p className="text-sm font-medium text-white">
                <Trans>Playback Error</Trans>
              </p>
              <p className="max-w-xs text-xs text-white/60">
                {i18n._(playbackErrorMessages[error])}
              </p>
            </div>
            <div className="flex flex-wrap justify-center gap-2">
              <Button size="sm" onClick={retry}>
                <Trans>Retry</Trans>
              </Button>
              {onRefreshSource && (
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => void refreshSource()}
                >
                  <Trans>Refresh stream URL</Trans>
                </Button>
              )}
              {sourceUrl && connection === 'direct' && (
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => chooseConnection('proxy')}
                >
                  <Trans>Try server proxy</Trans>
                </Button>
              )}
              {settingsContent && (
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => {
                    setSettingsTab('stream');
                    setSettingsOpen(true);
                  }}
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
