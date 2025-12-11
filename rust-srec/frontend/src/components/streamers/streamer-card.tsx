import { StreamerSchema } from '../../api/schemas';
import { z } from 'zod';
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
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from "../ui/popover";
import { Avatar, AvatarFallback, AvatarImage } from '../ui/avatar';
import { MoreHorizontal, RefreshCw, Trash, Edit, ExternalLink, Play, Pause, Video, Radio } from 'lucide-react';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { cn, getPlatformFromUrl } from '../../lib/utils';
import { useDownloadStore } from '../../store/downloads';
import { useShallow } from 'zustand/react/shallow';
import { ProgressIndicator } from './progress-indicator';

interface StreamerCardProps {
    streamer: z.infer<typeof StreamerSchema>;
    onDelete: (id: string) => void;
    onToggle: (id: string, enabled: boolean) => void;
    onCheck: (id: string) => void;
}

import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../ui/tooltip';

export function StreamerCard({ streamer, onDelete, onToggle, onCheck }: StreamerCardProps) {
    const isLive = streamer.state === 'LIVE';
    const isError = ['ERROR', 'FATAL_ERROR', 'OUT_OF_SPACE', 'TEMPORAL_DISABLED'].includes(streamer.state);

    // Query downloads for this streamer
    const downloads = useDownloadStore(useShallow(state => state.getDownloadsByStreamer(streamer.id)));
    const activeDownload = downloads[0]; // Show first active download

    const formatState = (state: string) => {
        if (state === 'NOT_LIVE') return 'Offline';
        if (state === 'LIVE') return 'Live';
        if (state === 'INSPECTING_LIVE') return 'Inspecting';
        if (state === 'OUT_OF_SCHEDULE') return 'Scheduled';
        return state.charAt(0).toUpperCase() + state.slice(1).toLowerCase().replace(/_/g, ' ');
    };

    const platform = getPlatformFromUrl(streamer.url);

    return (
        <Card className={cn(
            "group overflow-hidden transition-all duration-300 hover:shadow-lg hover:border-primary/20",
            !streamer.enabled && "opacity-60 grayscale hover:grayscale-0"
        )}>
            <CardHeader className="px-4 py-3">
                <div className="flex justify-between items-start">
                    <div className="space-y-3 w-full">
                        <div className="flex items-center justify-between w-full">
                            <div className="flex items-center gap-2">
                                <span className={cn(
                                    "flex h-2 w-2 rounded-full",
                                    isLive ? "bg-red-500 animate-pulse" :
                                        isError ? "bg-orange-500" : "bg-muted-foreground/30"
                                )} />
                                <Badge variant="outline" className={cn(
                                    "text-[10px] h-5 px-1.5 font-normal border-0",
                                    isLive ? "bg-red-50 text-red-600 dark:bg-red-900/20 dark:text-red-400" :
                                        isError ? "bg-orange-50 text-orange-600 dark:bg-orange-900/20 dark:text-orange-400" :
                                            "bg-muted text-muted-foreground"
                                )}>
                                    {formatState(streamer.state)}
                                </Badge>
                            </div>

                            <DropdownMenu>
                                <DropdownMenuTrigger asChild>
                                    <Button variant="ghost" size="icon" className="h-7 w-7 opacity-0 group-hover:opacity-100 transition-opacity -mr-2">
                                        <MoreHorizontal className="h-4 w-4 text-muted-foreground" />
                                    </Button>
                                </DropdownMenuTrigger>
                                <DropdownMenuContent align="end">
                                    <DropdownMenuLabel><Trans>Actions</Trans></DropdownMenuLabel>
                                    <DropdownMenuItem onClick={() => onCheck(streamer.id)}>
                                        <RefreshCw className="mr-2 h-4 w-4" /> <Trans>Check Now</Trans>
                                    </DropdownMenuItem>
                                    <DropdownMenuItem onClick={() => onToggle(streamer.id, !streamer.enabled)}>
                                        {streamer.enabled ? (
                                            <><Pause className="mr-2 h-4 w-4" /> <Trans>Disable</Trans></>
                                        ) : (
                                            <><Play className="mr-2 h-4 w-4" /> <Trans>Enable</Trans></>
                                        )}
                                    </DropdownMenuItem>
                                    <DropdownMenuSeparator />
                                    <DropdownMenuItem asChild>
                                        <Link to="/streamers/$id/edit" params={{ id: streamer.id }}>
                                            <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
                                        </Link>
                                    </DropdownMenuItem>
                                    <DropdownMenuItem onClick={() => onDelete(streamer.id)} className="text-red-600">
                                        <Trash className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                                    </DropdownMenuItem>
                                </DropdownMenuContent>
                            </DropdownMenu>
                        </div>

                        <div className="flex items-center gap-3">
                            <Avatar className="h-10 w-10 border border-border">
                                <AvatarImage src={streamer.avatar_url || undefined} alt={streamer.name} />
                                <AvatarFallback className="text-xs bg-muted text-muted-foreground">
                                    {streamer.name.substring(0, 2).toUpperCase()}
                                </AvatarFallback>
                            </Avatar>
                            <div className="min-w-0 flex-1">
                                <CardTitle className="text-base font-medium truncate leading-tight pr-2" title={streamer.name}>
                                    {streamer.name}
                                </CardTitle>

                                <div className="flex items-center gap-2 text-xs text-muted-foreground mt-1">
                                    <div className="flex items-center gap-1 bg-muted/40 px-1.5 py-0.5 rounded-md">
                                        {platform === 'twitch' ? <Video className="h-3 w-3" /> : <Radio className="h-3 w-3" />}
                                        <span className="capitalize">{platform}</span>
                                    </div>
                                    {streamer.consecutive_error_count > 0 && (
                                        streamer.last_error ? (
                                            <Popover>
                                                <PopoverTrigger asChild>
                                                    <Badge
                                                        variant="outline"
                                                        className="text-[10px] h-5 px-1 bg-red-50 text-red-600 border-red-100 cursor-pointer hover:bg-red-100 transition-colors"
                                                    >
                                                        {streamer.consecutive_error_count} err
                                                    </Badge>
                                                </PopoverTrigger>
                                                <PopoverContent className="w-80 p-0 overflow-hidden" align="start">
                                                    <div className="bg-red-50 border-b border-red-100 p-3">
                                                        <div className="flex items-center gap-2 text-red-700 font-medium text-sm">
                                                            <div className="h-2 w-2 rounded-full bg-red-500 animate-pulse" />
                                                            <Trans>Error Details</Trans>
                                                        </div>
                                                    </div>
                                                    <div className="p-3 bg-white text-sm text-muted-foreground whitespace-pre-wrap font-mono text-xs max-h-[300px] overflow-y-auto">
                                                        {streamer.last_error}
                                                    </div>
                                                </PopoverContent>
                                            </Popover>
                                        ) : (
                                            <Badge variant="outline" className="text-[10px] h-5 px-1 bg-red-50 text-red-600 border-red-100">
                                                {streamer.consecutive_error_count} err
                                            </Badge>
                                        )
                                    )}

                                    <TooltipProvider>
                                        <Tooltip>
                                            <TooltipTrigger asChild>
                                                <a href={streamer.url} target="_blank" rel="noopener noreferrer" className="ml-auto opacity-0 group-hover:opacity-100 transition-opacity">
                                                    <ExternalLink className="h-3 w-3 hover:text-primary transition-colors" />
                                                </a>
                                            </TooltipTrigger>
                                            <TooltipContent>
                                                <p className="max-w-[300px] break-all">{streamer.url}</p>
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
