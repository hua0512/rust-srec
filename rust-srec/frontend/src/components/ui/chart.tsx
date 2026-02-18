'use client';

import * as React from 'react';
import {
  ResponsiveContainer,
  Tooltip as RechartsTooltip,
  type TooltipProps as RechartsTooltipProps,
} from 'recharts';

import { cn } from '@/lib/utils';

export type ChartConfig = Record<
  string,
  {
    label?: React.ReactNode;
    color?: string;
  }
>;

const ChartContext = React.createContext<{ config: ChartConfig } | null>(null);

function useChart() {
  const context = React.useContext(ChartContext);
  if (!context) {
    throw new Error('useChart must be used within a <ChartContainer />');
  }
  return context;
}

interface ChartContainerProps extends React.ComponentProps<'div'> {
  config: ChartConfig;
  children: React.ReactNode;
}

function ChartContainer({
  id,
  className,
  config,
  children,
  ...props
}: ChartContainerProps) {
  const uniqueId = React.useId();
  const chartId = `chart-${id ?? uniqueId.replace(/:/g, '')}`;

  const style = React.useMemo(
    () =>
      Object.fromEntries(
        Object.entries(config)
          .filter(([, value]) => Boolean(value.color))
          .map(([key, value]) => [`--color-${key}`, value.color]),
      ) as React.CSSProperties,
    [config],
  );

  return (
    <ChartContext.Provider value={{ config }}>
      <div
        data-slot="chart"
        data-chart={chartId}
        className={cn(
          '[&_.recharts-cartesian-grid_line[stroke="#ccc"]]:stroke-border/50 [&_.recharts-curve.recharts-tooltip-cursor]:stroke-border [&_.recharts-reference-line_line]:stroke-border [&_.recharts-sector[stroke="#fff"]]:stroke-transparent [&_.recharts-text]:fill-muted-foreground [&_.recharts-xAxis_.recharts-cartesian-axis-tick_line]:stroke-border [&_.recharts-yAxis_.recharts-cartesian-axis-tick_line]:stroke-border [&_.recharts-layer]:outline-none',
          className,
        )}
        style={style}
        {...props}
      >
        <ResponsiveContainer>{children}</ResponsiveContainer>
      </div>
    </ChartContext.Provider>
  );
}

const ChartTooltip = RechartsTooltip;

type TooltipPayloadItem = {
  name?: string | number;
  value?: string | number;
  dataKey?: string | number;
  color?: string;
  payload?: Record<string, unknown>;
};

interface ChartTooltipContentProps {
  active?: boolean;
  payload?: TooltipPayloadItem[];
  label?: string | number;
  hideLabel?: boolean;
  className?: string;
  indicator?: 'line' | 'dot';
  formatter?: (
    value: string | number,
    name: string,
    item: TooltipPayloadItem,
    index: number,
    payload: TooltipPayloadItem[],
  ) => React.ReactNode;
}

function ChartTooltipContent({
  active,
  payload,
  label,
  hideLabel = false,
  className,
  indicator = 'dot',
  formatter,
}: ChartTooltipContentProps) {
  const { config } = useChart();

  if (!active || !payload || payload.length === 0) {
    return null;
  }

  return (
    <div
      className={cn(
        'grid min-w-[8rem] items-start gap-1.5 rounded-lg border border-border/50 bg-background/95 px-2.5 py-2 text-xs shadow-xl backdrop-blur-sm',
        className,
      )}
    >
      {!hideLabel && label != null ? (
        <div className="font-medium text-foreground">{label}</div>
      ) : null}
      <div className="grid gap-1">
        {payload.map((item, index) => {
          const dataKey = String(item.dataKey ?? item.name ?? index);
          const itemConfig = config[dataKey] ?? config.count;
          const markerColor = item.color ?? itemConfig?.color ?? 'currentColor';

          return (
            <div
              key={`${dataKey}-${index}`}
              className="flex items-center justify-between gap-2"
            >
              <div className="flex items-center gap-1.5">
                {indicator === 'line' ? (
                  <div
                    className="h-0.5 w-3"
                    style={{ backgroundColor: markerColor }}
                  />
                ) : (
                  <div
                    className="h-2 w-2 rounded-full"
                    style={{ backgroundColor: markerColor }}
                  />
                )}
                <span className="text-muted-foreground">
                  {itemConfig?.label ?? item.name ?? dataKey}
                </span>
              </div>
              <span className="font-mono font-medium text-foreground">
                {formatter
                  ? formatter(
                      item.value ?? '',
                      String(item.name ?? dataKey),
                      item,
                      index,
                      payload,
                    )
                  : item.value}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  useChart,
  type RechartsTooltipProps,
};
