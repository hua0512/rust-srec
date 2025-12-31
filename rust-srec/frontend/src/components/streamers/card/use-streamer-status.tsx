import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { formatDistanceToNow } from 'date-fns';
import {
  Clock,
  Calendar,
  AlertCircle,
  AlertTriangle,
  Activity,
  XCircle,
  Loader2,
} from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { StreamerSchema } from '@/api/schemas';
import { z } from 'zod';
import { useMemo, useState, useEffect } from 'react';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';

export function useStreamerStatus(streamer: z.infer<typeof StreamerSchema>) {
  const { i18n } = useLingui();

  // Client-side time to avoid hydration mismatch
  const [now, setNow] = useState<Date | null>(null);
  useEffect(() => {
    setNow(new Date());
  }, []);

  return useMemo(() => {
    const formatState = (state: string) => {
      if (state === 'NOT_LIVE') return <Trans>Offline</Trans>;
      if (state === 'LIVE') return <Trans>Live</Trans>;
      if (state === 'INSPECTING_LIVE') return <Trans>Inspecting</Trans>;
      if (state === 'OUT_OF_SCHEDULE') return <Trans>Scheduled</Trans>;
      if (state === 'OUT_OF_SPACE') return <Trans>Out of Space</Trans>;
      if (state === 'FATAL_ERROR') return <Trans>Fatal Error</Trans>;
      if (state === 'CANCELLED') return <Trans>Cancelled</Trans>;
      if (state === 'NOT_FOUND') return <Trans>Not Found</Trans>;
      if (state === 'TEMPORAL_DISABLED') return <Trans>Temporarily Paused</Trans>;
      if (state === 'ERROR') return <Trans>Error</Trans>;
      if (state === 'DISABLED') return <Trans>Disabled</Trans>;
      return (
        state.charAt(0).toUpperCase() +
        state.slice(1).toLowerCase().replace(/_/g, ' ')
      );
    };

    // Basic checks - during SSR use null for now
    const disabledUntil = streamer.disabled_until
      ? new Date(streamer.disabled_until)
      : null;
    // During SSR (now === null), rely only on state field
    const isTemporarilyPaused = now
      ? (disabledUntil && disabledUntil > now) ||
      streamer.state === 'TEMPORAL_DISABLED'
      : streamer.state === 'TEMPORAL_DISABLED';

    const stopStates = [
      'FATAL_ERROR',
      'NOT_FOUND',
      'OUT_OF_SPACE',
      'DISABLED',
      'CANCELLED',
      'ERROR',
    ];
    const isStopped = stopStates.includes(streamer.state);

    if (isTemporarilyPaused && disabledUntil) {
      return {
        label: <Trans>Temporarily Paused</Trans>,
        color:
          'bg-amber-500/10 text-amber-600 border-amber-500/20 hover:bg-amber-500/20 dark:text-amber-400 dark:border-amber-400/30',
        iconColor: 'bg-amber-500',
        pulsing: false,
        tooltip: (
          <StatusInfoTooltip
            theme="amber"
            icon={<Clock className="h-3.5 w-3.5" />}
            title={<Trans>Temporarily Paused</Trans>}
            subtitle={<Trans>Streamer is on cooldown</Trans>}
          >
            <div className="flex items-center justify-between text-xs p-2 rounded-md bg-muted/30 border border-border/40">
              <div className="flex items-center gap-1.5 text-muted-foreground">
                <Clock className="h-3.5 w-3.5 text-[var(--tooltip-theme-color)]" />
                <span className="font-medium">
                  <Trans>Resuming in</Trans>
                </span>
              </div>
              <Badge
                variant="secondary"
                className="font-mono font-bold text-[10px] bg-amber-500/10 text-amber-700 border-amber-500/20 dark:text-amber-400"
              >
                {formatDistanceToNow(disabledUntil, {
                  addSuffix: true,
                })}
              </Badge>
            </div>

            <div className="flex items-center justify-between text-xs px-1">
              <span className="text-muted-foreground">
                <Trans>Until</Trans>
              </span>
              <span className="font-mono text-xs font-medium text-foreground/80">
                {i18n.date(disabledUntil, { timeStyle: 'medium' })}
              </span>
            </div>

            {streamer.last_error && (
              <div className="space-y-2 pt-1">
                <p className="text-[10px] font-medium text-muted-foreground uppercase tracking-wider flex items-center gap-1.5 ml-0.5">
                  <AlertCircle className="h-3 w-3 opacity-70" />
                  <Trans>Last Error</Trans>
                </p>
                <div className="text-xs bg-muted/40 text-muted-foreground/90 p-2.5 rounded-md font-mono break-all border border-border/40 shadow-sm leading-relaxed max-h-[120px] overflow-y-auto">
                  {streamer.last_error}
                </div>
              </div>
            )}
          </StatusInfoTooltip>
        ),
      };
    }

    if (isStopped) {
      return {
        label: <Trans>Monitoring Stopped</Trans>,
        color:
          'bg-red-500/10 text-red-600 border-red-500/20 hover:bg-red-500/20 dark:text-red-400 dark:border-red-400/30',
        iconColor: 'bg-red-500',
        pulsing: false,
        tooltip: (
          <StatusInfoTooltip
            theme="red"
            icon={<XCircle className="h-3.5 w-3.5" />}
            title={<Trans>Monitoring Stopped</Trans>}
          >
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground">
                <Trans>Reason</Trans>
              </span>
              <Badge
                variant="outline"
                className="h-5 text-[10px] font-medium border-destructive/20 bg-destructive/5 text-destructive"
              >
                {formatState(streamer.state)}
              </Badge>
            </div>
            {streamer.last_error && (
              <div className="mt-1 pt-2 border-t border-border/10">
                <p className="text-[10px] text-muted-foreground mb-1.5 flex items-center gap-1">
                  <AlertTriangle className="h-3 w-3 text-[var(--tooltip-theme-color)]" />
                  <Trans>Error Details</Trans>
                </p>
                <div className="text-xs bg-muted/50 p-2 rounded-md font-mono text-muted-foreground break-all border border-border/50 leading-relaxed">
                  {streamer.last_error}
                </div>
              </div>
            )}
          </StatusInfoTooltip>
        ),
      };
    }

    // Standard states
    if (streamer.state === 'LIVE') {
      return {
        label: <Trans>Live</Trans>,
        color:
          'bg-red-500/10 text-red-500 hover:bg-red-500/20 border-red-500/20 animate-pulse',
        iconColor: 'bg-red-500',
        pingColor: 'bg-red-400',
        pulsing: true,
        variant: 'live',
        tooltip: null,
      };
    }

    if (streamer.state === 'INSPECTING_LIVE') {
      return {
        label: <Trans>Inspecting</Trans>,
        color:
          'bg-blue-500/10 text-blue-600 border-blue-500/20 hover:bg-blue-500/20 dark:text-blue-400 dark:border-blue-400/30',
        iconColor: 'bg-blue-500',
        pulsing: true,
        tooltip: (
          <StatusInfoTooltip
            theme="blue"
            icon={<Loader2 className="h-3.5 w-3.5 animate-spin" />}
            title={<Trans>Inspecting</Trans>}
          >
            <div className="p-2 text-xs font-medium">
              <Trans>Checking stream status...</Trans>
            </div>
          </StatusInfoTooltip>
        ),
      };
    }

    if (streamer.state === 'OUT_OF_SCHEDULE') {
      return {
        label: <Trans>Scheduled</Trans>,
        color:
          'bg-violet-500/10 text-violet-600 border-violet-500/20 hover:bg-violet-500/20 dark:text-violet-400 dark:border-violet-400/30',
        iconColor: 'bg-violet-500',
        pulsing: false,
        tooltip: (
          <StatusInfoTooltip
            theme="violet"
            icon={<Calendar className="h-3.5 w-3.5" />}
            title={<Trans>Outside Schedule</Trans>}
          >
            <div className="text-xs text-muted-foreground leading-relaxed">
              <Trans>
                This streamer is currently live, but outside your configured
                recording schedule.
              </Trans>
            </div>
            <div className="flex items-center gap-1.5 text-xs text-violet-600 dark:text-violet-400 font-medium">
              <Calendar className="h-3.5 w-3.5 text-[var(--tooltip-theme-color)]" />
              <Trans>Recording will start when schedule allows</Trans>
            </div>
          </StatusInfoTooltip>
        ),
      };
    }

    if (streamer.state === 'NOT_LIVE') {
      const lastLiveDate = streamer.last_live_time
        ? new Date(streamer.last_live_time)
        : null;
      const hasValidLastLive =
        lastLiveDate &&
        !isNaN(lastLiveDate.getTime()) &&
        lastLiveDate.getFullYear() > 1970;

      return {
        label: <Trans>Offline</Trans>,
        color:
          'bg-muted/50 text-muted-foreground border-border/50 hover:bg-muted/80',
        iconColor: 'bg-muted-foreground/40',
        pulsing: false,
        tooltip: hasValidLastLive ? (
          <StatusInfoTooltip
            theme="slate"
            icon={<Activity className="h-3.5 w-3.5" />}
            title={<Trans>Offline</Trans>}
          >
            <div className="flex justify-between items-center text-xs p-2 rounded-md bg-muted/40 border border-border/40">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <Activity className="h-3 w-3 text-[var(--tooltip-theme-color)]" />
                <Trans>Last Activity</Trans>
              </span>
              <span className="font-mono font-medium text-foreground">
                {i18n.date(lastLiveDate, {
                  dateStyle: 'medium',
                  timeStyle: 'medium',
                })}
              </span>
            </div>
          </StatusInfoTooltip>
        ) : null,
      };
    }

    // Fallback
    return {
      label: formatState(streamer.state),
      color: 'bg-muted text-muted-foreground border-transparent',
      iconColor: 'bg-muted-foreground',
      pulsing: false,
      tooltip: null,
    };
  }, [streamer, i18n, now]);
}
