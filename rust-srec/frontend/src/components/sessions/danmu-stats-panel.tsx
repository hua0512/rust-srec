import { type ReactNode, memo, useCallback, useMemo, useRef } from 'react';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { motion } from 'motion/react';
import {
  Activity,
  Clock,
  MessageCircleMore,
  RotateCcw,
  Users,
} from 'lucide-react';
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  Brush,
  CartesianGrid,
  ReferenceLine,
  XAxis,
  YAxis,
} from 'recharts';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';
import { containerVariants, itemVariants } from '@/lib/animation';

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
import { cn } from '@/lib/utils';

// ---------------------------------------------------------------------------
// Module-level constants – never recreated
// ---------------------------------------------------------------------------

const MAX_CHART_POINTS = 150;
const AREA_MARGIN = { top: 8, right: 8, left: 0, bottom: 8 } as const;
const BAR_MARGIN = { top: 4, right: 8, left: 0, bottom: 4 } as const;
const BAR_RADIUS_H: [number, number, number, number] = [0, 4, 4, 0];
const TICK_SM = { fontSize: 12 };
const ACTIVE_DOT = { r: 4, strokeWidth: 0 };
const CURSOR_STYLE = {
  stroke: 'var(--color-count)',
  strokeWidth: 1,
  strokeDasharray: '4 4',
};

const CHART_TOOLTIP = (
  <ChartTooltipContent
    indicator="dot"
    formatter={(value) => Number(value).toLocaleString()}
  />
);

const TIMELINE_TOOLTIP = (
  <ChartTooltipContent
    indicator="line"
    formatter={(value) => Number(value).toLocaleString()}
  />
);

/**
 * Peak-preserving downsample: divides data into equal-sized buckets and keeps
 * the point with the highest count in each bucket. This retains visual spikes
 * while dramatically reducing SVG path complexity for brush interactions.
 */
function downsampleTimeseries<T extends { count: number }>(
  data: T[],
  maxPoints: number,
): T[] {
  if (data.length <= maxPoints) return data;

  const bucketSize = data.length / maxPoints;
  const result: T[] = [data[0]];

  for (let i = 1; i < maxPoints - 1; i++) {
    const start = Math.floor(i * bucketSize);
    const end = Math.min(Math.floor((i + 1) * bucketSize), data.length);
    let best = data[start];
    for (let j = start + 1; j < end; j++) {
      if (data[j].count > best.count) best = data[j];
    }
    result.push(best);
  }

  result.push(data[data.length - 1]);
  return result;
}

// ---------------------------------------------------------------------------
// PanelShell
// ---------------------------------------------------------------------------

function PanelShell({ children }: { children: ReactNode }) {
  return (
    <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xl relative">
      <div className="absolute top-0 right-0 w-40 h-40 bg-primary/5 rounded-full blur-3xl -mr-12 -mt-12 pointer-events-none" />
      <CardHeader className="pb-3">
        <CardTitle className="text-lg font-semibold flex items-center gap-2">
          <Activity className="h-5 w-5 text-chart-1" />
          <Trans>Danmu Statistics</Trans>
        </CardTitle>
      </CardHeader>
      {children}
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Public entry – handles loading / error / unavailable without any hooks
// ---------------------------------------------------------------------------

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
  if (isLoading) {
    return (
      <PanelShell>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
            <Skeleton className="h-20 rounded-xl" />
            <Skeleton className="h-20 rounded-xl" />
            <Skeleton className="h-20 rounded-xl" />
            <Skeleton className="h-20 rounded-xl" />
          </div>
          <Skeleton className="h-56 rounded-xl" />
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            <Skeleton className="h-44 rounded-xl" />
            <Skeleton className="h-44 rounded-xl" />
          </div>
        </CardContent>
      </PanelShell>
    );
  }

  if (isUnavailable) {
    return (
      <PanelShell>
        <CardContent className="text-sm text-muted-foreground">
          <Trans>Danmu statistics are not available for this session.</Trans>
        </CardContent>
      </PanelShell>
    );
  }

  if (isError || !stats) {
    return (
      <PanelShell>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">
            <Trans>Failed to load danmu statistics.</Trans>
          </p>
          <Button type="button" variant="outline" size="sm" onClick={onRetry}>
            <RotateCcw className="mr-2 h-4 w-4" />
            <Trans>Retry</Trans>
          </Button>
        </CardContent>
      </PanelShell>
    );
  }

  // Only mount the heavy inner component when we actually have data
  return <DanmuStatsPanelInner stats={stats} />;
}

// ---------------------------------------------------------------------------
// Inner – only mounted when `stats` is defined; all hooks live here
// ---------------------------------------------------------------------------

function DanmuStatsPanelInner({ stats }: { stats: SessionDanmuStatistics }) {
  const { i18n } = useLingui();
  const prefersReducedMotion = usePrefersReducedMotion();
  const shouldAnimate = !prefersReducedMotion;

  const hasBrushed = useRef(false);
  const onBrushChange = useCallback(() => {
    hasBrushed.current = true;
  }, []);

  // -- derived data --------------------------------------------------------

  const chartData = useMemo(() => {
    const sorted = [...stats.danmu_rate_timeseries].sort((a, b) => a.ts - b.ts);
    return downsampleTimeseries(sorted, MAX_CHART_POINTS).map((point) => ({
      label: i18n.date(new Date(point.ts), {
        hour: '2-digit',
        minute: '2-digit',
      }),
      count: point.count,
    }));
  }, [i18n, stats]);

  const { peakPerMinute, averagePerMinute } = useMemo(() => {
    if (stats.danmu_rate_timeseries.length === 0) {
      return { peakPerMinute: 0, averagePerMinute: 0 };
    }
    let max = 0;
    let sum = 0;
    for (const point of stats.danmu_rate_timeseries) {
      if (point.count > max) max = point.count;
      sum += point.count;
    }
    return {
      peakPerMinute: max,
      averagePerMinute: Math.round(sum / stats.danmu_rate_timeseries.length),
    };
  }, [stats]);

  const topTalkers = useMemo(() => stats.top_talkers.slice(0, 6), [stats]);
  const topWords = useMemo(() => stats.word_frequency.slice(0, 10), [stats]);

  const talkersChartData = useMemo(
    () =>
      topTalkers.map((item) => ({
        name: item.username || item.user_id,
        message_count: item.message_count,
      })),
    [topTalkers],
  );

  // -- chart configs -------------------------------------------------------

  const chartConfig = useMemo(
    () =>
      ({
        count: {
          label: i18n._(msg`Danmu/min`),
          color: 'hsl(var(--chart-1))',
        },
      }) satisfies ChartConfig,
    [i18n],
  );

  const talkersChartConfig = useMemo(
    () =>
      ({
        message_count: {
          label: i18n._(msg`Messages`),
          color: 'hsl(var(--chart-2))',
        },
      }) satisfies ChartConfig,
    [i18n],
  );

  const wordsChartConfig = useMemo(
    () =>
      ({
        count: {
          label: i18n._(msg`Count`),
          color: 'hsl(var(--chart-3))',
        },
      }) satisfies ChartConfig,
    [i18n],
  );

  const referenceLineLabel = useMemo(
    () => ({
      value: i18n._(msg`avg ${averagePerMinute}`),
      position: 'insideTopRight' as const,
      fontSize: 10,
      fill: 'var(--color-count)',
      opacity: 0.7,
    }),
    [i18n, averagePerMinute],
  );

  // -- render --------------------------------------------------------------

  return (
    <motion.div
      initial={shouldAnimate ? 'hidden' : false}
      animate="visible"
      variants={containerVariants}
    >
      <PanelShell>
        <CardContent className="space-y-6">
          {/* ── Metric grid ─────────────────────────────────────── */}
          <motion.div
            variants={containerVariants}
            className="grid grid-cols-2 md:grid-cols-4 gap-3"
          >
            <MetricBlock
              icon={<MessageCircleMore className="h-4 w-4 text-blue-500" />}
              label={i18n._(msg`Total Danmu`)}
              value={stats.total_danmus.toLocaleString()}
              accent="bg-blue-500/10"
            />
            <MetricBlock
              icon={<Activity className="h-4 w-4 text-orange-500" />}
              label={i18n._(msg`Peak / min`)}
              value={peakPerMinute.toLocaleString()}
              accent="bg-orange-500/10"
            />
            <MetricBlock
              icon={<Users className="h-4 w-4 text-purple-500" />}
              label={i18n._(msg`Top Talkers`)}
              value={topTalkers.length.toString()}
              accent="bg-purple-500/10"
            />
            <MetricBlock
              icon={<MessageCircleMore className="h-4 w-4 text-emerald-500" />}
              label={i18n._(msg`Tracked Words`)}
              value={stats.word_frequency.length.toString()}
              accent="bg-emerald-500/10"
            />
          </motion.div>

          {/* ── Timeline chart ──────────────────────────────────── */}
          <motion.div variants={itemVariants} className="space-y-2">
            <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground flex items-center gap-1.5">
              <Clock className="h-3.5 w-3.5" />
              <Trans>Timeline</Trans>
            </p>
            <ChartContainer config={chartConfig} className="h-64 w-full">
              <AreaChart data={chartData} margin={AREA_MARGIN}>
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
                  cursor={CURSOR_STYLE}
                  content={TIMELINE_TOOLTIP}
                />
                {averagePerMinute > 0 && (
                  <ReferenceLine
                    y={averagePerMinute}
                    stroke="var(--color-count)"
                    strokeDasharray="6 3"
                    strokeOpacity={0.5}
                    label={referenceLineLabel}
                  />
                )}
                <Area
                  type="monotone"
                  dataKey="count"
                  stroke="var(--color-count)"
                  fill="url(#fillDanmu)"
                  strokeWidth={2}
                  activeDot={ACTIVE_DOT}
                  isAnimationActive={shouldAnimate && !hasBrushed.current}
                  animationDuration={700}
                  animationEasing="ease-out"
                />
                {chartData.length > 20 && (
                  <Brush
                    dataKey="label"
                    height={24}
                    stroke="var(--color-count)"
                    travellerWidth={8}
                    fill="transparent"
                    onChange={onBrushChange}
                  />
                )}
              </AreaChart>
            </ChartContainer>
          </motion.div>

          {/* ── Rank charts ─────────────────────────────────────── */}
          <motion.div
            variants={containerVariants}
            className="grid grid-cols-1 gap-4 md:grid-cols-2"
          >
            <RankBarChart
              icon={<Users className="h-3.5 w-3.5 text-purple-500" />}
              title={i18n._(msg`Top Talkers`)}
              emptyLabel={i18n._(msg`No talker data`)}
              config={talkersChartConfig}
              data={talkersChartData}
              dataKey="message_count"
              categoryKey="name"
              yAxisWidth={80}
              barSize={20}
              rowHeight={36}
              shouldAnimate={shouldAnimate}
            />
            <RankBarChart
              icon={
                <MessageCircleMore className="h-3.5 w-3.5 text-emerald-500" />
              }
              title={i18n._(msg`Top Words`)}
              emptyLabel={i18n._(msg`No word data`)}
              config={wordsChartConfig}
              data={topWords}
              dataKey="count"
              categoryKey="word"
              yAxisWidth={72}
              barSize={18}
              rowHeight={32}
              shouldAnimate={shouldAnimate}
            />
          </motion.div>
        </CardContent>
      </PanelShell>
    </motion.div>
  );
}

// ---------------------------------------------------------------------------
// RankBarChart – memo'd so it doesn't re-render during brush drags
// ---------------------------------------------------------------------------

interface RankBarChartProps {
  icon: ReactNode;
  title: string;
  emptyLabel: string;
  config: ChartConfig;
  data: Record<string, unknown>[];
  dataKey: string;
  categoryKey: string;
  yAxisWidth: number;
  barSize: number;
  rowHeight: number;
  shouldAnimate: boolean;
}

const RankBarChart = memo(function RankBarChart({
  icon,
  title,
  emptyLabel,
  config,
  data,
  dataKey,
  categoryKey,
  yAxisWidth,
  barSize,
  rowHeight,
  shouldAnimate,
}: RankBarChartProps) {
  const fillVar = `var(--color-${dataKey})`;
  const height = data.length * rowHeight + 16;

  return (
    <motion.div
      variants={itemVariants}
      className="rounded-xl border border-border/50 bg-background/30 p-4 hover:border-border transition-colors"
    >
      <p className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground flex items-center gap-1.5">
        {icon}
        {title}
      </p>
      {data.length === 0 ? (
        <p className="text-xs text-muted-foreground">{emptyLabel}</p>
      ) : (
        <ChartContainer config={config} className="w-full" style={{ height }}>
          <BarChart data={data} layout="vertical" margin={BAR_MARGIN}>
            <YAxis
              dataKey={categoryKey}
              type="category"
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              width={yAxisWidth}
              tick={TICK_SM}
            />
            <XAxis type="number" hide />
            <ChartTooltip cursor={false} content={CHART_TOOLTIP} />
            <Bar
              dataKey={dataKey}
              fill={fillVar}
              radius={BAR_RADIUS_H}
              isAnimationActive={shouldAnimate}
              animationDuration={500}
              animationEasing="ease-out"
              barSize={barSize}
            />
          </BarChart>
        </ChartContainer>
      )}
    </motion.div>
  );
});

// ---------------------------------------------------------------------------
// MetricBlock
// ---------------------------------------------------------------------------

interface MetricBlockProps {
  icon: ReactNode;
  label: string;
  value: string;
  accent: string;
}

function MetricBlock({ icon, label, value, accent }: MetricBlockProps) {
  return (
    <motion.div
      variants={itemVariants}
      className="group relative overflow-hidden rounded-xl border border-border/50 bg-background/40 p-3 hover:bg-background/60 hover:border-border transition-colors"
    >
      <div
        className={cn(
          'absolute inset-0 opacity-0 group-hover:opacity-100 transition-opacity duration-300',
          accent,
        )}
      />
      <div className="relative">
        <div className="mb-1.5 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
          <div
            className={cn(
              'h-6 w-6 rounded-md flex items-center justify-center shrink-0',
              accent,
            )}
          >
            {icon}
          </div>
          {label}
        </div>
        <p className="font-mono text-lg font-bold tracking-tight text-foreground">
          {value}
        </p>
      </div>
    </motion.div>
  );
}
