import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { Badge } from '@/components/ui/badge';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { motion } from 'motion/react';
import { cn, getProxiedUrl } from '@/lib/utils';
import { formatBytes } from '@/lib/format';
import {
  Monitor,
  Activity,
  Calendar,
  Video,
  Play,
  Timer,
  Zap,
  HardDrive,
} from 'lucide-react';
import { getMediaUrl } from '@/lib/url';
import { isPlayable } from '@/lib/media';
import type { SessionDanmuStatistics } from '@/api/schemas';
import { DanmuStatsPanel } from './danmu-stats-panel';

interface OverviewTabProps {
  session: any;
  duration: string;
  outputs: any[];
  onPlay: (output: any) => void;
  token?: string;
  danmuStats: SessionDanmuStatistics | undefined;
  isDanmuStatsLoading: boolean;
  isDanmuStatsError: boolean;
  isDanmuStatsUnavailable: boolean;
  onRetryDanmuStats: () => void;
}

export function OverviewTab({
  session,
  duration,
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

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 }}
      className="space-y-8"
    >
      <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xl">
        <div className="absolute top-0 right-0 p-32 bg-primary/5 rounded-full blur-3xl -mr-16 -mt-16 pointer-events-none" />
        <CardContent className="p-4 md:p-8">
          <div className="flex flex-col lg:flex-row gap-6 md:gap-8 items-start lg:items-center justify-between">
            <div className="flex items-center gap-4 md:gap-6">
              <div className="relative group">
                <div className="absolute -inset-1 bg-gradient-to-br from-primary to-blue-600 rounded-full opacity-20 group-hover:opacity-40 transition-opacity blur-md" />
                <Avatar className="h-16 w-16 md:h-24 md:w-24 border-2 md:border-4 border-background shadow-xl relative">
                  {getProxiedUrl(session.streamer_avatar) && (
                    <AvatarImage
                      src={getProxiedUrl(session.streamer_avatar)}
                      alt={session.streamer_name}
                      className="object-cover"
                    />
                  )}
                  <AvatarFallback className="text-xl md:text-3xl font-bold bg-muted text-muted-foreground">
                    {session.streamer_name?.substring(0, 2).toUpperCase() ||
                      '??'}
                  </AvatarFallback>
                </Avatar>
                <div className="absolute bottom-1 right-1 h-4 w-4 md:h-5 md:w-5 bg-green-500 border-2 md:border-4 border-white dark:border-zinc-900 rounded-full" />
              </div>

              <div className="space-y-2">
                <div className="flex items-center gap-3">
                  <Badge
                    variant="secondary"
                    className="bg-primary/10 text-primary hover:bg-primary/20 transition-colors"
                  >
                    <Trans>STREAMER</Trans>
                  </Badge>
                  <p className="text-xs font-mono text-muted-foreground">
                    ID: {session.streamer_id}
                  </p>
                </div>
                <h2 className="text-2xl md:text-3xl lg:text-4xl font-bold tracking-tight text-foreground truncate max-w-[200px] sm:max-w-none">
                  {session.streamer_name}
                </h2>
              </div>
            </div>

            <div className="grid grid-cols-2 md:grid-cols-2 lg:grid-cols-3 gap-3 md:gap-4 w-full lg:w-auto min-w-0 md:min-w-[300px]">
              <StatBox
                icon={HardDrive}
                label={<Trans>Total Size</Trans>}
                value={formatBytes(session.total_size_bytes)}
                color="text-blue-500"
                bg="bg-blue-500/10"
              />
              <StatBox
                icon={Video}
                label={<Trans>Outputs</Trans>}
                value={session.output_count}
                color="text-purple-500"
                bg="bg-purple-500/10"
              />
              <StatBox
                icon={Activity}
                label={<Trans>Danmus</Trans>}
                value={session.danmu_count?.toLocaleString() || '0'}
                color="text-orange-500"
                bg="bg-orange-500/10"
                className="col-span-2 lg:col-span-1"
              />
            </div>
          </div>
        </CardContent>
      </Card>

      <div className="grid grid-cols-1 xl:grid-cols-3 gap-8">
        <div className="xl:col-span-2 space-y-8">
          <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xs h-full min-h-[400px] flex flex-col">
            <CardHeader className="pb-3 border-b border-border/40">
              <CardTitle className="text-lg font-semibold flex items-center gap-2">
                <Monitor className="h-5 w-5 text-primary" />
                <Trans>Session Preview</Trans>
              </CardTitle>
            </CardHeader>
            <CardContent className="p-0 flex-1 flex flex-col">
              <div className="aspect-video bg-muted/30 flex items-center justify-center relative group overflow-hidden flex-1 min-h-[300px]">
                {thumbnailUrl ? (
                  <>
                    <img
                      src={thumbnailUrl}
                      alt={i18n._(msg`Session thumbnail`)}
                      className="w-full h-full object-cover transition-transform duration-700 group-hover:scale-105"
                    />
                    <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-black/20 to-transparent opacity-60 group-hover:opacity-80 transition-opacity" />
                    <motion.div
                      whileHover={{ scale: 1.1 }}
                      className="absolute inset-0 flex items-center justify-center"
                      onClick={() => playableOutput && onPlay(playableOutput)}
                    >
                      {playableOutput ? (
                        <button className="relative group/play cursor-pointer">
                          <div className="absolute inset-0 bg-white/30 rounded-full blur-xl group-hover/play:bg-white/50 transition-colors" />
                          <div className="relative h-14 w-14 md:h-20 md:w-20 rounded-full bg-white/10 backdrop-blur-md border border-white/20 flex items-center justify-center shadow-2xl group-hover/play:scale-105 transition-transform">
                            <Play className="h-6 w-6 md:h-8 md:w-8 text-white fill-white translate-x-1" />
                          </div>
                        </button>
                      ) : (
                        <div className="px-4 py-2 rounded-full bg-black/40 backdrop-blur-md border border-white/10 text-white/80 font-medium text-sm">
                          <Trans>Preview Unavailable</Trans>
                        </div>
                      )}
                    </motion.div>
                  </>
                ) : (
                  <div className="flex flex-col items-center gap-4 text-muted-foreground/30 p-4 text-center">
                    <Monitor className="h-16 w-16" />
                    <p className="font-medium text-sm text-muted-foreground/50">
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
                      {duration.split(' ')[0]}
                    </span>
                    <span className="text-lg font-medium text-muted-foreground">
                      {duration.split(' ').slice(1).join(' ')}
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

      <DanmuStatsPanel
        stats={danmuStats}
        isLoading={isDanmuStatsLoading}
        isError={isDanmuStatsError}
        isUnavailable={isDanmuStatsUnavailable}
        onRetry={onRetryDanmuStats}
      />
    </motion.div>
  );
}

function StatBox({ icon: Icon, label, value, color, bg }: any) {
  return (
    <div className="flex items-center gap-4 bg-background/50 border border-border/50 p-3 rounded-xl hover:bg-background/80 transition-colors">
      <div
        className={cn(
          'h-10 w-10 rounded-lg flex items-center justify-center shrink-0',
          bg,
        )}
      >
        <Icon className={cn('h-5 w-5', color)} />
      </div>
      <div>
        <p className="text-[10px] font-bold uppercase tracking-wider text-muted-foreground truncate">
          {label}
        </p>
        <p className="text-xs md:text-sm font-bold">{value}</p>
      </div>
    </div>
  );
}

function TimeBlock({ label, date, icon: Icon, delay }: any) {
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
          ? i18n.date(new Date(date), {
              hour: 'numeric',
              minute: 'numeric',
              second: 'numeric',
            })
          : '-'}
      </div>
      <div className="text-[10px] text-muted-foreground/60">
        {date ? (
          i18n.date(new Date(date), {
            month: 'short',
            day: 'numeric',
            year: 'numeric',
          })
        ) : (
          <Trans>In active</Trans>
        )}
      </div>
    </motion.div>
  );
}
