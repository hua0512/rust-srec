import { type ComponentType, useEffect, useMemo, useState } from 'react';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { motion } from 'motion/react';
import { Activity, MessageCircleMore, RotateCcw, Users } from 'lucide-react';
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from 'recharts';

import type { SessionDanmuStatistics } from '@/api/schemas';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart';
import { Skeleton } from '@/components/ui/skeleton';

interface DanmuStatsPanelProps {
  stats: SessionDanmuStatistics | undefined;
  isLoading: boolean;
  isError: boolean;
  isUnavailable: boolean;
  onRetry: () => void;
}

export function DanmuStatsPanel({
  stats,
  isLoading,
  isError,
  isUnavailable,
  onRetry,
}: DanmuStatsPanelProps) {
  const { i18n } = useLingui();
  const [prefersReducedMotion, setPrefersReducedMotion] = useState(false);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }

    const media = window.matchMedia('(prefers-reduced-motion: reduce)');
    const sync = () => setPrefersReducedMotion(media.matches);

    sync();
    media.addEventListener('change', sync);
    return () => media.removeEventListener('change', sync);
  }, []);

  const shouldAnimate = !prefersReducedMotion;

  const chartData = useMemo(() => {
    if (!stats) {
      return [] as { label: string; count: number }[];
    }

    return [...stats.danmu_rate_timeseries]
      .sort((a, b) => a.ts - b.ts)
      .map((point) => ({
        label: i18n.date(new Date(point.ts), {
          hour: '2-digit',
          minute: '2-digit',
        }),
        count: point.count,
      }));
  }, [i18n, stats]);

  const peakPerMinute = useMemo(() => {
    if (!stats || stats.danmu_rate_timeseries.length === 0) {
      return 0;
    }

    let maxValue = 0;
    for (const point of stats.danmu_rate_timeseries) {
      if (point.count > maxValue) {
        maxValue = point.count;
      }
    }
    return maxValue;
  }, [stats]);

  const topTalkers = stats?.top_talkers.slice(0, 6) ?? [];
  const topWords = stats?.word_frequency.slice(0, 10) ?? [];

  const chartConfig = {
    count: {
      label: i18n._(msg`Danmu/min`),
      color: 'hsl(var(--chart-1))',
    },
  } satisfies ChartConfig;

  if (isLoading) {
    return (
      <Card className="bg-card/40 border-border/50">
        <CardHeader>
          <CardTitle className="text-lg flex items-center gap-2">
            <Activity className="h-5 w-5 text-chart-1" />
            <Trans>Danmu Statistics</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-2 gap-3">
            <Skeleton className="h-16 rounded-xl" />
            <Skeleton className="h-16 rounded-xl" />
            <Skeleton className="h-16 rounded-xl" />
            <Skeleton className="h-16 rounded-xl" />
          </div>
          <Skeleton className="h-48 rounded-xl" />
        </CardContent>
      </Card>
    );
  }

  if (isUnavailable) {
    return (
      <Card className="bg-card/40 border-border/50">
        <CardHeader>
          <CardTitle className="text-lg flex items-center gap-2">
            <Activity className="h-5 w-5 text-chart-1" />
            <Trans>Danmu Statistics</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          <Trans>Danmu statistics are not available for this session.</Trans>
        </CardContent>
      </Card>
    );
  }

  if (isError || !stats) {
    return (
      <Card className="bg-card/40 border-border/50">
        <CardHeader>
          <CardTitle className="text-lg flex items-center gap-2">
            <Activity className="h-5 w-5 text-chart-1" />
            <Trans>Danmu Statistics</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">
            <Trans>Failed to load danmu statistics.</Trans>
          </p>
          <Button type="button" variant="outline" size="sm" onClick={onRetry}>
            <RotateCcw className="mr-2 h-4 w-4" />
            <Trans>Retry</Trans>
          </Button>
        </CardContent>
      </Card>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: shouldAnimate ? 0.35 : 0 }}
    >
      <Card className="bg-card/40 border-border/50">
        <CardHeader>
          <CardTitle className="text-lg flex items-center gap-2">
            <Activity className="h-5 w-5 text-chart-1" />
            <Trans>Danmu Statistics</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-5">
          <div className="grid grid-cols-2 gap-3">
            <MetricBlock
              icon={MessageCircleMore}
              label={i18n._(msg`Total Danmu`)}
              value={stats.total_danmus.toLocaleString()}
            />
            <MetricBlock
              icon={Activity}
              label={i18n._(msg`Peak / min`)}
              value={peakPerMinute.toLocaleString()}
            />
            <MetricBlock
              icon={Users}
              label={i18n._(msg`Top Talkers`)}
              value={topTalkers.length.toString()}
            />
            <MetricBlock
              icon={MessageCircleMore}
              label={i18n._(msg`Tracked Words`)}
              value={stats.word_frequency.length.toString()}
            />
          </div>

          <div className="space-y-2">
            <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              <Trans>Timeline</Trans>
            </p>
            <ChartContainer config={chartConfig} className="h-52 w-full">
              <AreaChart
                data={chartData}
                margin={{ top: 8, right: 8, left: 0, bottom: 8 }}
              >
                <defs>
                  <linearGradient id="fillDanmu" x1="0" y1="0" x2="0" y2="1">
                    <stop
                      offset="5%"
                      stopColor="var(--color-count)"
                      stopOpacity={0.35}
                    />
                    <stop
                      offset="95%"
                      stopColor="var(--color-count)"
                      stopOpacity={0.05}
                    />
                  </linearGradient>
                </defs>
                <CartesianGrid vertical={false} strokeDasharray="3 3" />
                <XAxis
                  dataKey="label"
                  tickLine={false}
                  axisLine={false}
                  minTickGap={28}
                  tickMargin={8}
                />
                <YAxis
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  width={38}
                />
                <ChartTooltip
                  cursor={false}
                  content={
                    <ChartTooltipContent
                      indicator="line"
                      formatter={(value) => Number(value).toLocaleString()}
                    />
                  }
                />
                <Area
                  type="monotone"
                  dataKey="count"
                  stroke="var(--color-count)"
                  fill="url(#fillDanmu)"
                  strokeWidth={2}
                  activeDot={{ r: 4, strokeWidth: 0 }}
                  isAnimationActive={shouldAnimate}
                  animationDuration={700}
                  animationEasing="ease-out"
                />
              </AreaChart>
            </ChartContainer>
          </div>

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <SimpleRankList
              title={i18n._(msg`Top Talkers`)}
              emptyLabel={i18n._(msg`No talker data`)}
              rows={topTalkers.map((item) => ({
                label: item.username || item.user_id,
                value: item.message_count.toLocaleString(),
              }))}
            />
            <SimpleRankList
              title={i18n._(msg`Top Words`)}
              emptyLabel={i18n._(msg`No word data`)}
              rows={topWords.map((item) => ({
                label: item.word,
                value: item.count.toLocaleString(),
              }))}
            />
          </div>
        </CardContent>
      </Card>
    </motion.div>
  );
}

interface MetricBlockProps {
  icon: ComponentType<{ className?: string }>;
  label: string;
  value: string;
}

function MetricBlock({ icon: Icon, label, value }: MetricBlockProps) {
  return (
    <div className="rounded-xl border border-border/60 bg-background/40 p-3">
      <div className="mb-1 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted-foreground">
        <Icon className="h-3.5 w-3.5 text-chart-1" />
        {label}
      </div>
      <p className="font-mono text-sm font-semibold text-foreground">{value}</p>
    </div>
  );
}

interface SimpleRankListProps {
  title: string;
  emptyLabel: string;
  rows: Array<{ label: string; value: string }>;
}

function SimpleRankList({ title, emptyLabel, rows }: SimpleRankListProps) {
  return (
    <div className="rounded-xl border border-border/60 bg-background/30 p-3">
      <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {title}
      </p>
      {rows.length === 0 ? (
        <p className="text-xs text-muted-foreground">{emptyLabel}</p>
      ) : (
        <div className="space-y-1.5">
          {rows.map((row, index) => (
            <div
              key={`${row.label}-${index}`}
              className="flex items-center justify-between gap-3 text-sm"
            >
              <span className="truncate text-foreground/90">{row.label}</span>
              <span className="font-mono text-xs text-muted-foreground">
                {row.value}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
