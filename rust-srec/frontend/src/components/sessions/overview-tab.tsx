import React, { Suspense, useEffect, useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { StreamerAvatar } from '@/components/shared/streamer-avatar';
import { Badge } from '@/components/ui/badge';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';
import { STAT_GRID } from '@/lib/grid';
import { formatBytes, formatTimecode } from '@/lib/format';
import {
  Monitor,
  Activity,
  Calendar,
  Video,
  Play,
  Timer,
  Zap,
  HardDrive,
  type LucideIcon,
} from 'lucide-react';
import { getMediaUrl } from '@/lib/url';
import { isPlayable } from '@/lib/media';
import type { z } from 'zod';
import type { SessionDanmuStatistics, SessionSchema } from '@/api/schemas';
import type { MediaOutput } from '@/api/schemas/system';
import { StatTile } from '@/components/shared/stat-tile';
import { Skeleton } from '@/components/ui/skeleton';
import { formatDate, formatLocalizedDuration } from '@/lib/datetime';

const DanmuStatsPanel = React.lazy(() =>
  import('./danmu-stats-panel').then((module) => ({
    default: module.DanmuStatsPanel,
  })),
);

/**
 * The recorded length; a session still recording has no `duration_secs` yet,
 * so its length counts up from `startTime`. The clock starts after mount so
 * the server-rendered markup and the first client render agree.
 */
function SessionDuration({
  durationSecs,
  startTime,
  endTime,
}: {
  durationSecs: number | null | undefined;
  startTime: string | null | undefined;
  endTime: string | null | undefined;
}) {
  const { i18n } = useLingui();
  const isLive = !endTime;
  const [now, setNow] = useState<number | null>(null);
  useEffect(() => {
    if (!isLive) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [isLive]);

  const seconds =
    typeof durationSecs === 'number' && durationSecs > 0
      ? durationSecs
      : isLive && startTime && now !== null
        ? Math.max(0, (now - Date.parse(startTime)) / 1000)
        : null;
  return formatLocalizedDuration(seconds, i18n.locale);
}

interface OverviewTabProps {
  session: z.infer<typeof SessionSchema>;
  outputs: MediaOutput[];
  onPlay: (output: MediaOutput) => void;
  token?: string;
  danmuStats: SessionDanmuStatistics | undefined;
  isDanmuStatsLoading: boolean;
  isDanmuStatsError: boolean;
  isDanmuStatsUnavailable: boolean;
  onRetryDanmuStats: () => void;
}

export function OverviewTab({
  session,
  outputs,
  onPlay,
  token,
  danmuStats,
  isDanmuStatsLoading,
  isDanmuStatsError,
  isDanmuStatsUnavailable,
  onRetryDanmuStats,
}: OverviewTabProps) {
  const { i18n } = useLingui();
  const thumbnailUrl = getMediaUrl(session.thumbnail_url, token);

  const playableOutput = outputs.find(isPlayable);
  const previewDurationSecs =
    (playableOutput?.duration_secs && playableOutput.duration_secs > 0
      ? playableOutput.duration_secs
      : null) ??
    (session.duration_secs && session.duration_secs > 0
      ? session.duration_secs
      : null);

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 }}
      className="@container space-y-8"
    >
      {/* Columns follow this tab's own width: the page pads itself heavily and sits beside the
          app sidebar, so viewport breakpoints overestimate the room. */}
      <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xl">
        <div className="absolute top-0 right-0 p-32 bg-primary/5 rounded-full blur-3xl -mr-16 -mt-16 pointer-events-none" />
        <CardContent className="p-4 md:p-8">
          <div className="flex flex-col @4xl:flex-row gap-6 md:gap-8 items-start @4xl:items-center justify-between">
            <div className="flex min-w-0 max-w-full items-center gap-4 md:gap-6">
              <div className="relative group">
                <div className="absolute -inset-1 bg-gradient-to-br from-primary to-blue-600 rounded-full opacity-20 group-hover:opacity-40 transition-opacity blur-md" />
                <StreamerAvatar
                  src={session.streamer_avatar}
                  name={session.streamer_name}
                  className="h-16 w-16 md:h-24 md:w-24 border-2 md:border-4 border-background shadow-xl relative"
                  fallbackClassName="text-xl md:text-3xl font-bold"
                />
                {!session.end_time && (
                  <div className="absolute bottom-1 right-1 h-4 w-4 md:h-5 md:w-5 bg-green-500 border-2 md:border-4 border-white dark:border-zinc-900 rounded-full" />
                )}
              </div>

              <div className="min-w-0 space-y-2">
                <div className="flex items-center gap-3">
                  <Badge
                    variant="secondary"
                    className="bg-primary/10 text-primary hover:bg-primary/20 transition-colors"
                  >
                    <Trans>STREAMER</Trans>
                  </Badge>
                  {session.streamer_id && (
                    <p className="text-xs font-mono text-muted-foreground">
                      ID: {session.streamer_id}
                    </p>
                  )}
                </div>
                <h2 className="text-2xl md:text-3xl lg:text-4xl font-bold tracking-tight text-foreground truncate max-w-[200px] sm:max-w-none">
                  {session.streamer_name}
                </h2>
              </div>
            </div>

            <div className="grid grid-cols-2 @4xl:grid-cols-3 gap-3 md:gap-4 w-full @4xl:w-auto @4xl:shrink-0 min-w-0 md:min-w-[300px]">
              <StatTile
                icon={<HardDrive className="h-4 w-4 text-blue-500" />}
                label={<Trans>Total Size</Trans>}
                value={formatBytes(session.total_size_bytes)}
                accent="bg-blue-500/10"
              />
              <StatTile
                icon={<Video className="h-4 w-4 text-purple-500" />}
                label={<Trans>Outputs</Trans>}
                value={session.output_count}
                accent="bg-purple-500/10"
              />
              <StatTile
                icon={<Activity className="h-4 w-4 text-orange-500" />}
                label={<Trans>Danmus</Trans>}
                value={(session.danmu_count ?? 0).toLocaleString(i18n.locale)}
                accent="bg-orange-500/10"
                className="col-span-2 @4xl:col-span-1"
              />
            </div>
          </div>
        </CardContent>
      </Card>

      <div className="grid grid-cols-1 @4xl:grid-cols-3 gap-8">
        <div className="@4xl:col-span-2 space-y-8">
          <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xs p-0 gap-0">
            <CardContent className="p-0">
              <div className="aspect-video w-full bg-muted/30 flex items-center justify-center relative group/preview overflow-hidden">
                <div
                  className={cn(
                    'pointer-events-none absolute top-3.5 left-3.5 z-10 inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium backdrop-blur-md',
                    thumbnailUrl
                      ? 'bg-black/55 text-white/95 ring-1 ring-white/15 shadow-sm'
                      : 'bg-background/80 text-foreground ring-1 ring-border/60',
                  )}
                >
                  <Monitor
                    className={cn(
                      'h-3.5 w-3.5',
                      thumbnailUrl ? 'text-white/80' : 'text-primary',
                    )}
                  />
                  <Trans>Session Preview</Trans>
                </div>

                {thumbnailUrl ? (
                  <>
                    {/* The largest element of the default tab, so it is
                        fetched ahead of the avatars and other images. */}
                    <img
                      src={thumbnailUrl}
                      alt={i18n._(msg`Session thumbnail`)}
                      fetchPriority="high"
                      className={cn(
                        'w-full h-full object-cover',
                        playableOutput &&
                          'transition-transform duration-500 ease-out group-hover/preview:scale-[1.03] motion-reduce:transition-none motion-reduce:group-hover/preview:scale-100',
                      )}
                    />
                    {playableOutput ? (
                      <>
                        <div className="pointer-events-none absolute inset-0 bg-radial from-transparent to-black/35 opacity-30 transition-opacity duration-300 group-hover/preview:opacity-75" />
                        <button
                          type="button"
                          aria-label={i18n._(msg`Play`)}
                          className="absolute inset-0 flex items-center justify-center group/play cursor-pointer focus-visible:outline-none"
                          onClick={() => onPlay(playableOutput)}
                        >
                          <span className="flex h-14 w-14 md:h-16 md:w-16 items-center justify-center rounded-full bg-black/65 text-white ring-1 ring-white/30 backdrop-blur-md shadow-[0_12px_32px_rgba(0,0,0,0.45)] transition-all duration-200 group-hover/play:scale-110 group-hover/play:bg-primary group-hover/play:text-primary-foreground group-hover/play:ring-white/50 group-focus-visible/play:ring-4 group-focus-visible/play:ring-white/70 motion-reduce:transition-none motion-reduce:group-hover/play:scale-100">
                            <Play className="h-6 w-6 md:h-7 md:w-7 fill-current translate-x-0.5" />
                          </span>
                        </button>
                      </>
                    ) : (
                      <div className="absolute inset-0 flex items-center justify-center bg-black/25">
                        <div className="px-4 py-2 rounded-full bg-black/60 backdrop-blur-md ring-1 ring-white/15 text-white/90 font-medium text-sm shadow-lg">
                          <Trans>Preview Unavailable</Trans>
                        </div>
                      </div>
                    )}
                    {previewDurationSecs != null && (
                      <div className="pointer-events-none absolute bottom-3.5 right-3.5 z-10 rounded-md bg-black/65 px-2 py-1 font-mono text-xs font-medium tabular-nums text-white/95 ring-1 ring-white/10 backdrop-blur-md">
                        {formatTimecode(previewDurationSecs)}
                      </div>
                    )}
                  </>
                ) : (
                  <div className="flex flex-col items-center gap-3 text-muted-foreground/40 p-6 text-center">
                    <Monitor className="h-12 w-12" />
                    <p className="font-medium text-sm text-muted-foreground/70">
                      <Trans>No preview generated</Trans>
                    </p>
                  </div>
                )}
              </div>
            </CardContent>
          </Card>
        </div>

        <div className="space-y-8 flex flex-col">
          <Card className="bg-gradient-to-b from-card/50 to-card/30 backdrop-blur-sm border-border/40 shadow-sm relative overflow-hidden">
            <div className="absolute top-0 right-0 w-32 h-32 bg-primary/5 rounded-full blur-2xl -mr-10 -mt-10" />
            <CardHeader className="pb-2">
              <CardTitle className="text-lg font-semibold flex items-center gap-2">
                <Zap className="h-5 w-5 text-yellow-500" />
                <Trans>Performance</Trans>
              </CardTitle>
            </CardHeader>
            <CardContent className="p-6">
              <div className="flex flex-col gap-6">
                <div className="bg-background/40 rounded-2xl p-6 border border-border/50 relative overflow-hidden group">
                  <div className="absolute inset-0 bg-gradient-to-r from-transparent via-primary/5 to-transparent -translate-x-full group-hover:translate-x-full transition-transform duration-1000" />
                  <span className="text-muted-foreground text-xs font-bold uppercase tracking-widest">
                    <Trans>Session Duration</Trans>
                  </span>
                  <div className="mt-2 flex items-baseline gap-2">
                    <span className="text-4xl font-extrabold tracking-tight text-foreground">
                      <SessionDuration
                        durationSecs={session.duration_secs}
                        startTime={session.start_time}
                        endTime={session.end_time}
                      />
                    </span>
                  </div>
                </div>

                <div className="grid grid-cols-2 gap-4">
                  <TimeBlock
                    label={i18n._(msg`Started`)}
                    date={session.start_time}
                    icon={Calendar}
                    delay={0}
                  />
                  <TimeBlock
                    label={i18n._(msg`Ended`)}
                    date={session.end_time}
                    icon={Timer}
                    delay={0.1}
                  />
                </div>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>

      <Suspense
        fallback={
          <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xl relative">
            <div className="absolute top-0 right-0 w-40 h-40 bg-primary/5 rounded-full blur-3xl -mr-12 -mt-12 pointer-events-none" />
            <CardHeader className="pb-3">
              <CardTitle className="text-lg font-semibold flex items-center gap-2">
                <Activity className="h-5 w-5 text-chart-1" />
                <Trans>Danmu Statistics</Trans>
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className={cn(STAT_GRID, 'gap-3')}>
                <Skeleton className="h-20 rounded-xl" />
                <Skeleton className="h-20 rounded-xl" />
                <Skeleton className="h-20 rounded-xl" />
                <Skeleton className="h-20 rounded-xl" />
              </div>
              <Skeleton className="h-56 rounded-xl" />
            </CardContent>
          </Card>
        }
      >
        <DanmuStatsPanel
          stats={danmuStats}
          isLoading={isDanmuStatsLoading}
          isError={isDanmuStatsError}
          isUnavailable={isDanmuStatsUnavailable}
          onRetry={onRetryDanmuStats}
        />
      </Suspense>
    </motion.div>
  );
}

function TimeBlock({
  label,
  date,
  icon: Icon,
  delay,
}: {
  label: string;
  date: string | null | undefined;
  icon: LucideIcon;
  delay: number;
}) {
  const { i18n } = useLingui();
  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 + delay }}
      className="flex flex-col gap-1.5 p-3 rounded-xl bg-muted/20 border border-transparent hover:border-border/50 transition-colors"
    >
      <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
        <Icon className="h-3.5 w-3.5" />
        {label}
      </div>
      <div className="font-mono text-sm font-semibold">
        {date
          ? formatDate(i18n.locale, new Date(date), {
              hour: 'numeric',
              minute: 'numeric',
              second: 'numeric',
            })
          : '-'}
      </div>
      <div className="text-[10px] text-muted-foreground/60">
        {date ? (
          formatDate(i18n.locale, new Date(date), {
            month: 'short',
            day: 'numeric',
            year: 'numeric',
          })
        ) : (
          <Trans>In progress</Trans>
        )}
      </div>
    </motion.div>
  );
}
