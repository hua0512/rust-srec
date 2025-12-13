import { StreamerSchema } from '../../api/schemas';
import { z } from 'zod';
import { formatDistanceToNow } from 'date-fns';
import { Card, CardHeader, CardTitle } from '../ui/card';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuLabel,
    DropdownMenuSeparator,
    DropdownMenuTrigger,
} from '../ui/dropdown-menu';

import { Avatar, AvatarFallback, AvatarImage } from '../ui/avatar';
import {
    MoreHorizontal,
    RefreshCw,
    Trash,
    Edit,
    ExternalLink,
    Play,
    Pause,
    Video,
    Radio,
    Clock,
    Calendar,
    AlertCircle,
    AlertTriangle,
    Activity
} from 'lucide-react';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { cn, getPlatformFromUrl } from '../../lib/utils';
import { useDownloadStore } from '../../store/downloads';
import { useShallow } from 'zustand/react/shallow';
import { ProgressIndicator } from './progress-indicator';

import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../ui/tooltip';

interface StreamerCardProps {
    streamer: z.infer<typeof StreamerSchema>;
    onDelete: (id: string) => void;
    onToggle: (id: string, enabled: boolean) => void;
    onCheck: (id: string) => void;
}

const StatusBadge = ({ status }: { status: any }) => (
    <TooltipProvider delayDuration={200}>
        <Tooltip>
            <TooltipTrigger asChild>
                <div className={cn(
                    "flex items-center gap-1.5 h-6 px-2 pr-2.5 rounded-full border text-[10px] font-medium transition-all cursor-help select-none",
                    status.color
                )}>
                    {status.variant === 'live' ? (
                        <span className="relative flex h-2 w-2 mr-1.5">
                            <span className={cn("animate-ping absolute inline-flex h-full w-full rounded-full opacity-75", status.pingColor)}></span>
                            <span className={cn("relative inline-flex rounded-full h-2 w-2", status.iconColor)}></span>
                        </span>
                    ) : (
                        <span className={cn(
                            "h-1.5 w-1.5 rounded-full min-w-[6px]",
                            status.iconColor,
                            status.pulsing && "animate-pulse shadow-[0_0_8px_rgba(239,68,68,0.6)]"
                        )} />
                    )}
                    {status.label}
                </div>
            </TooltipTrigger>
            {status.tooltip && (
                <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden" side="bottom" align="start">
                    {status.tooltip}
                </TooltipContent>
            )}
        </Tooltip>
    </TooltipProvider>
);

export function StreamerCard({ streamer, onDelete, onToggle, onCheck }: StreamerCardProps) {

    const formatState = (state: string) => {
        if (state === 'NOT_LIVE') return <Trans>Offline</Trans>;
        if (state === 'LIVE') return <Trans>Live</Trans>;
        if (state === 'INSPECTING_LIVE') return <Trans>Inspecting</Trans>;
        if (state === 'OUT_OF_SCHEDULE') return <Trans>Scheduled</Trans>;
        return state.charAt(0).toUpperCase() + state.slice(1).toLowerCase().replace(/_/g, ' ');
    };

    // Basic checks
    const now = new Date();
    const disabledUntil = streamer.disabled_until ? new Date(streamer.disabled_until) : null;
    const isTemporarilyPaused = disabledUntil && disabledUntil > now;

    const stopStates = ['FATAL_ERROR', 'NOT_FOUND', 'OUT_OF_SPACE', 'DISABLED', 'CANCELLED', 'ERROR'];
    const isStopped = stopStates.includes(streamer.state);

    const getStatusDisplay = () => {
        if (isTemporarilyPaused && disabledUntil) {
            return {
                label: <Trans>Temporarily Paused</Trans>,
                color: "bg-amber-500/10 text-amber-600 border-amber-500/20 hover:bg-amber-500/20 dark:text-amber-400 dark:border-amber-400/30",
                iconColor: "bg-amber-500",
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
                                    <span><Trans>Resuming in</Trans></span>
                                </div>
                                <span className="font-medium text-amber-600 dark:text-amber-400 tabular-nums">
                                    {formatDistanceToNow(disabledUntil, { addSuffix: true })}
                                </span>
                            </div>
                            <div className="flex items-center justify-between text-xs">
                                <div className="flex items-center gap-1.5 text-muted-foreground">
                                    <Calendar className="h-3.5 w-3.5" />
                                    <span><Trans>Until</Trans></span>
                                </div>
                                <span className="font-medium tabular-nums">{disabledUntil.toLocaleTimeString()}</span>
                            </div>
                            {streamer.last_error && (
                                <div className="mt-2 pt-2 border-t border-border/10">
                                    <p className="text-[10px] text-muted-foreground mb-1.5 flex items-center gap-1"><AlertCircle className="h-3 w-3" /><Trans>Last Error</Trans></p>
                                    <div className="text-xs bg-destructive/5 text-destructive/90 p-2 rounded-md font-mono break-all border border-destructive/10 leading-relaxed">
                                        {streamer.last_error}
                                    </div>
                                </div>
                            )}
                        </div>
                    </div>
                )
            };
        }

        if (isStopped) {
            return {
                label: <Trans>Monitoring Stopped</Trans>,
                color: "bg-red-500/10 text-red-600 border-red-500/20 hover:bg-red-500/20 dark:text-red-400 dark:border-red-400/30",
                iconColor: "bg-red-500",
                pulsing: false,
                tooltip: (
                    <div className="flex flex-col gap-2.5 p-3 min-w-[240px]">
                        <div className="flex items-center gap-2 pb-2 border-b border-border/10">
                            <div className="h-2 w-2 rounded-full bg-destructive shadow-[0_0_8px_rgba(239,68,68,0.5)]" />
                            <span className="font-semibold text-sm text-destructive">Monitoring Stopped</span>
                        </div>
                        <div className="space-y-2">
                            <div className="flex items-center justify-between text-xs">
                                <span className="text-muted-foreground"><Trans>Reason</Trans></span>
                                <Badge variant="outline" className="h-5 text-[10px] font-medium border-destructive/20 bg-destructive/5 text-destructive">
                                    {formatState(streamer.state)}
                                </Badge>
                            </div>
                            {streamer.last_error && (
                                <div className="mt-1 pt-2 border-t border-border/10">
                                    <p className="text-[10px] text-muted-foreground mb-1.5 flex items-center gap-1"><AlertTriangle className="h-3 w-3" /><Trans>Error Details</Trans></p>
                                    <div className="text-xs bg-muted/50 p-2 rounded-md font-mono text-muted-foreground break-all border border-border/50 leading-relaxed">
                                        {streamer.last_error}
                                    </div>
                                </div>
                            )}
                        </div>
                    </div>
                )
            };
        }

        // Standard states
        if (streamer.state === 'LIVE') {
            return {
                label: <Trans>Live</Trans>,
                color: "bg-red-500/10 text-red-500 hover:bg-red-500/20 border-red-500/20 animate-pulse",
                iconColor: "bg-red-500",
                pingColor: "bg-red-400",
                pulsing: true,
                variant: 'live',
                tooltip: null
            };
        }

        if (streamer.state === 'INSPECTING_LIVE') {
            return {
                label: <Trans>Inspecting</Trans>,
                color: "bg-blue-500/10 text-blue-600 border-blue-500/20 hover:bg-blue-500/20 dark:text-blue-400 dark:border-blue-400/30",
                iconColor: "bg-blue-500",
                pulsing: true,
                tooltip: (
                    <div className="p-2 text-xs font-medium">
                        <Trans>Checking stream status...</Trans>
                    </div>
                )
            };
        }

        if (streamer.state === 'NOT_LIVE') {
            const lastLiveDate = streamer.last_live_time ? new Date(streamer.last_live_time) : null;
            const hasValidLastLive = lastLiveDate && !isNaN(lastLiveDate.getTime()) && lastLiveDate.getFullYear() > 1970;

            return {
                label: <Trans>Offline</Trans>,
                color: "bg-muted/50 text-muted-foreground border-border/50 hover:bg-muted/80",
                iconColor: "bg-muted-foreground/40",
                pulsing: false,
                tooltip: hasValidLastLive ? (
                    <div className="p-2.5 flex flex-col gap-1.5 min-w-[180px]">
                        <div className="flex items-center gap-2 text-xs font-semibold text-muted-foreground border-b border-border/10 pb-1.5">
                            <Activity className="h-3.5 w-3.5" />
                            <Trans>Last Activity</Trans>
                        </div>
                        <div className="flex justify-between items-center text-xs">
                            <span className="font-medium text-foreground">{lastLiveDate?.toLocaleString()}</span>
                        </div>
                    </div>
                ) : null
            };
        }

        // Fallback
        return {
            label: formatState(streamer.state),
            color: "bg-muted text-muted-foreground border-transparent",
            iconColor: "bg-muted-foreground",
            pulsing: false,
            tooltip: null
        };
    };

    // Query downloads for this streamer
    const downloads = useDownloadStore(useShallow(state => state.getDownloadsByStreamer(streamer.id)));
    const activeDownload = downloads[0]; // Show first active download

    const status = getStatusDisplay();
    const platform = getPlatformFromUrl(streamer.url);

    return (
        <Card className={cn(
            "group overflow-hidden transition-all duration-300 hover:shadow-xl hover:border-primary/20",
            !streamer.enabled ? "opacity-60 grayscale-[0.8] hover:grayscale-0 hover:opacity-100" : ""
        )}>
            <CardHeader className="px-4 py-3">
                <div className="flex justify-between items-start">
                    <div className="space-y-3 w-full">
                        <div className="flex items-center justify-between w-full">
                            <div className="flex items-center gap-2">
                                <StatusBadge status={status} />
                            </div>

                            <DropdownMenu>
                                <DropdownMenuTrigger asChild>
                                    <Button variant="ghost" size="icon" className="h-7 w-7 opacity-0 group-hover:opacity-100 transition-opacity -mr-2 text-muted-foreground hover:text-foreground">
                                        <MoreHorizontal className="h-4 w-4" />
                                    </Button>
                                </DropdownMenuTrigger>
                                <DropdownMenuContent align="end" className="w-56">
                                    <DropdownMenuLabel><Trans>Actions</Trans></DropdownMenuLabel>
                                    <DropdownMenuItem onClick={() => onCheck(streamer.id)} className="cursor-pointer group">
                                        <RefreshCw className="mr-2 h-4 w-4 text-primary group-hover:text-primary" />
                                        <span className="group-hover:text-primary transition-colors"><Trans>Check Now</Trans></span>
                                    </DropdownMenuItem>
                                    <DropdownMenuItem
                                        onClick={() => onToggle(streamer.id, !streamer.enabled)}
                                        className="cursor-pointer group"
                                    >
                                        {streamer.enabled ? (
                                            <>
                                                <Pause className="mr-2 h-4 w-4 text-orange-500 group-hover:text-orange-600 dark:text-orange-400 dark:group-hover:text-orange-300" />
                                                <span className="text-orange-600 group-hover:text-orange-700 dark:text-orange-400 dark:group-hover:text-orange-300 transition-colors"><Trans>Disable</Trans></span>
                                            </>
                                        ) : (
                                            <>
                                                <Play className="mr-2 h-4 w-4 text-green-500 group-hover:text-green-600 dark:text-green-400 dark:group-hover:text-green-300" />
                                                <span className="text-green-600 group-hover:text-green-700 dark:text-green-400 dark:group-hover:text-green-300 transition-colors"><Trans>Enable</Trans></span>
                                            </>
                                        )}
                                    </DropdownMenuItem>
                                    <DropdownMenuSeparator />
                                    <DropdownMenuItem asChild className="cursor-pointer group">
                                        <Link to="/streamers/$id/edit" params={{ id: streamer.id }}>
                                            <Edit className="mr-2 h-4 w-4 text-blue-500 group-hover:text-blue-600 dark:text-blue-400 dark:group-hover:text-blue-300" />
                                            <span className="text-blue-600 group-hover:text-blue-700 dark:text-blue-400 dark:group-hover:text-blue-300 transition-colors"><Trans>Edit</Trans></span>
                                        </Link>
                                    </DropdownMenuItem>
                                    <DropdownMenuItem onClick={() => onDelete(streamer.id)} className="cursor-pointer group focus:bg-red-50 dark:focus:bg-red-950/20">
                                        <Trash className="mr-2 h-4 w-4 text-red-500 group-hover:text-red-600" />
                                        <span className="text-red-600 group-hover:text-red-700"><Trans>Delete</Trans></span>
                                    </DropdownMenuItem>
                                </DropdownMenuContent>
                            </DropdownMenu>
                        </div>

                        <div className="flex items-center gap-3">
                            <Avatar className="h-10 w-10 border border-border/60 shadow-sm transition-transform group-hover:scale-105">
                                <AvatarImage src={streamer.avatar_url || undefined} alt={streamer.name} />
                                <AvatarFallback className="text-xs bg-muted text-muted-foreground font-medium">
                                    {streamer.name.substring(0, 2).toUpperCase()}
                                </AvatarFallback>
                            </Avatar>
                            <div className="min-w-0 flex-1">
                                <CardTitle className="text-base font-semibold truncate leading-tight pr-2 tracking-tight" title={streamer.name}>
                                    {streamer.name}
                                </CardTitle>

                                <div className="flex items-center gap-2 text-xs text-muted-foreground mt-1.5">
                                    <div className={cn(
                                        "flex items-center gap-1 px-1.5 py-0.5 rounded-md border border-border/40 transition-colors group-hover:border-border/60",
                                        platform === 'twitch' ? "bg-[#6441a5]/10 text-[#6441a5] border-[#6441a5]/10 dark:text-[#a970ff] dark:bg-[#a970ff]/10" :
                                            platform === 'youtube' ? "bg-[#FF0000]/10 text-[#FF0000] border-[#FF0000]/10" :
                                                "bg-muted/40"
                                    )}>
                                        {platform === 'twitch' ? <Video className="h-3 w-3" /> : <Radio className="h-3 w-3" />}
                                        <span className="capitalize font-medium">{platform}</span>
                                    </div>

                                    {/* Consecutive errors are now part of valid status, but if we want to show counting attempts when NOT stopped/paused yet */}
                                    {streamer.consecutive_error_count > 0 && !isTemporarilyPaused && !isStopped && (
                                        <Badge variant="outline" className="text-[10px] h-5 px-1 bg-orange-500/10 text-orange-600 border-orange-500/20">
                                            {streamer.consecutive_error_count} err
                                        </Badge>
                                    )}

                                    <TooltipProvider>
                                        <Tooltip>
                                            <TooltipTrigger asChild>
                                                <a href={streamer.url} target="_blank" rel="noopener noreferrer" className="ml-auto opacity-0 group-hover:opacity-100 transition-opacity p-1 hover:bg-muted rounded-full">
                                                    <ExternalLink className="h-3 w-3 hover:text-primary transition-colors" />
                                                </a>
                                            </TooltipTrigger>
                                            <TooltipContent>
                                                <p className="max-w-[300px] break-all font-mono text-xs">{streamer.url}</p>
                                            </TooltipContent>
                                        </Tooltip>
                                    </TooltipProvider>
                                </div>
                            </div>
                        </div>

                        {/* Download progress indicator */}
                        {activeDownload && (
                            <ProgressIndicator progress={activeDownload} compact />
                        )}
                    </div>
                </div>
            </CardHeader>
        </Card >
    );
}
