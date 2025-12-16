import { Trans } from '@lingui/react/macro';
import { formatDistanceToNow } from 'date-fns';
import {
    Clock,
    Calendar,
    AlertCircle,
    AlertTriangle,
    Activity,
} from 'lucide-react';
import { Badge } from '../../ui/badge';
import { StreamerSchema } from '../../../api/schemas';
import { z } from 'zod';
import { useMemo } from 'react';

export function useStreamerStatus(streamer: z.infer<typeof StreamerSchema>) {
    return useMemo(() => {
        const formatState = (state: string) => {
            if (state === 'NOT_LIVE') return <Trans>Offline</Trans>;
            if (state === 'LIVE') return <Trans>Live</Trans>;
            if (state === 'INSPECTING_LIVE') return <Trans>Inspecting</Trans>;
            if (state === 'OUT_OF_SCHEDULE') return <Trans>Scheduled</Trans>;
            return (
                state.charAt(0).toUpperCase() +
                state.slice(1).toLowerCase().replace(/_/g, ' ')
            );
        };

        // Basic checks
        const now = new Date();
        const disabledUntil = streamer.disabled_until
            ? new Date(streamer.disabled_until)
            : null;
        const isTemporarilyPaused =
            (disabledUntil && disabledUntil > now) ||
            streamer.state === 'TEMPORAL_DISABLED';

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
                    <div className="flex flex-col gap-2.5 p-3 min-w-[240px]">
                        <div className="flex items-center gap-2 pb-2 border-b border-border/10">
                            <div className="h-2 w-2 rounded-full bg-amber-500 shadow-[0_0_8px_rgba(245,158,11,0.5)]" />
                            <span className="font-semibold text-sm">Temporarily Paused</span>
                        </div>
                        <div className="space-y-2">
                            <div className="flex items-center justify-between text-xs group">
                                <div className="flex items-center gap-1.5 text-muted-foreground">
                                    <Clock className="h-3.5 w-3.5" />
                                    <span>
                                        <Trans>Resuming in</Trans>
                                    </span>
                                </div>
                                <span className="font-medium text-amber-600 dark:text-amber-400 tabular-nums">
                                    {formatDistanceToNow(disabledUntil, { addSuffix: true })}
                                </span>
                            </div>
                            <div className="flex items-center justify-between text-xs">
                                <div className="flex items-center gap-1.5 text-muted-foreground">
                                    <Calendar className="h-3.5 w-3.5" />
                                    <span>
                                        <Trans>Until</Trans>
                                    </span>
                                </div>
                                <span className="font-medium tabular-nums">
                                    {disabledUntil.toLocaleTimeString()}
                                </span>
                            </div>
                            {streamer.last_error && (
                                <div className="mt-2 pt-2 border-t border-border/10">
                                    <p className="text-[10px] text-muted-foreground mb-1.5 flex items-center gap-1">
                                        <AlertCircle className="h-3 w-3" />
                                        <Trans>Last Error</Trans>
                                    </p>
                                    <div className="text-xs bg-destructive/5 text-destructive/90 p-2 rounded-md font-mono break-all border border-destructive/10 leading-relaxed">
                                        {streamer.last_error}
                                    </div>
                                </div>
                            )}
                        </div>
                    </div>
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
                    <div className="flex flex-col gap-2.5 p-3 min-w-[240px]">
                        <div className="flex items-center gap-2 pb-2 border-b border-border/10">
                            <div className="h-2 w-2 rounded-full bg-destructive shadow-[0_0_8px_rgba(239,68,68,0.5)]" />
                            <span className="font-semibold text-sm text-destructive">
                                Monitoring Stopped
                            </span>
                        </div>
                        <div className="space-y-2">
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
                                        <AlertTriangle className="h-3 w-3" />
                                        <Trans>Error Details</Trans>
                                    </p>
                                    <div className="text-xs bg-muted/50 p-2 rounded-md font-mono text-muted-foreground break-all border border-border/50 leading-relaxed">
                                        {streamer.last_error}
                                    </div>
                                </div>
                            )}
                        </div>
                    </div>
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
                    <div className="p-2 text-xs font-medium">
                        <Trans>Checking stream status...</Trans>
                    </div>
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
                    <div className="flex flex-col gap-2 p-3 min-w-[220px]">
                        <div className="flex items-center gap-2 pb-2 border-b border-border/10">
                            <div className="h-2 w-2 rounded-full bg-violet-500 shadow-[0_0_8px_rgba(139,92,246,0.5)]" />
                            <span className="font-semibold text-sm">
                                <Trans>Outside Schedule</Trans>
                            </span>
                        </div>
                        <div className="text-xs text-muted-foreground leading-relaxed">
                            <Trans>
                                This streamer is currently live, but outside
                                your configured recording schedule.
                            </Trans>
                        </div>
                        <div className="flex items-center gap-1.5 text-xs text-violet-600 dark:text-violet-400 font-medium">
                            <Calendar className="h-3.5 w-3.5" />
                            <Trans>Recording will start when schedule allows</Trans>
                        </div>
                    </div>
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
                    <div className="p-2.5 flex flex-col gap-1.5 min-w-[180px]">
                        <div className="flex items-center gap-2 text-xs font-semibold text-muted-foreground border-b border-border/10 pb-1.5">
                            <Activity className="h-3.5 w-3.5" />
                            <Trans>Last Activity</Trans>
                        </div>
                        <div className="flex justify-between items-center text-xs">
                            <span className="font-medium text-foreground">
                                {lastLiveDate?.toLocaleString()}
                            </span>
                        </div>
                    </div>
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
    }, [streamer]);
}
