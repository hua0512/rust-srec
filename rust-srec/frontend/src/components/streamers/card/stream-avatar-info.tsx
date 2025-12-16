import { Avatar, AvatarFallback, AvatarImage } from '../../ui/avatar';
import { CardTitle } from '../../ui/card';
import {
    Tooltip,
    TooltipContent,
    TooltipProvider,
    TooltipTrigger,
} from '../../ui/tooltip';
import { Badge } from '../../ui/badge';
import {
    Video,
    Radio,
    ExternalLink,
    AlertTriangle,
    Activity,
} from 'lucide-react';
import { cn, getPlatformFromUrl } from '../../../lib/utils';
import { Trans } from '@lingui/react/macro';
import { z } from 'zod';
import { StreamerSchema } from '../../../api/schemas';
import { StatusInfoTooltip } from './status-info-tooltip';

interface StreamAvatarInfoProps {
    streamer: z.infer<typeof StreamerSchema>;
}

export const StreamAvatarInfo = ({ streamer }: StreamAvatarInfoProps) => {
    const platform = getPlatformFromUrl(streamer.url);
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

    return (
        <div className="flex items-center gap-3">
            <Avatar className="h-10 w-10 border border-border/60 shadow-sm transition-transform group-hover:scale-105">
                <AvatarImage
                    src={streamer.avatar_url || undefined}
                    alt={streamer.name}
                />
                <AvatarFallback className="text-xs bg-muted text-muted-foreground font-medium">
                    {streamer.name.substring(0, 2).toUpperCase()}
                </AvatarFallback>
            </Avatar>
            <div className="min-w-0 flex-1">
                <CardTitle
                    className="text-base font-semibold truncate leading-tight pr-2 tracking-tight"
                    title={streamer.name}
                >
                    {streamer.name}
                </CardTitle>

                <div className="flex items-center gap-2 text-xs text-muted-foreground mt-1.5">
                    <div
                        className={cn(
                            'flex items-center gap-1 px-1.5 py-0.5 rounded-md border border-border/40 transition-colors group-hover:border-border/60',
                            platform === 'twitch'
                                ? 'bg-[#6441a5]/10 text-[#6441a5] border-[#6441a5]/10 dark:text-[#a970ff] dark:bg-[#a970ff]/10'
                                : platform === 'youtube'
                                    ? 'bg-[#FF0000]/10 text-[#FF0000] border-[#FF0000]/10'
                                    : 'bg-muted/40',
                        )}
                    >
                        {platform === 'twitch' ? (
                            <Video className="h-3 w-3" />
                        ) : (
                            <Radio className="h-3 w-3" />
                        )}
                        <span className="capitalize font-medium">{platform}</span>
                    </div>

                    {/* Consecutive errors */}
                    {streamer.consecutive_error_count > 0 &&
                        !isTemporarilyPaused &&
                        !isStopped && (
                            <TooltipProvider>
                                <Tooltip delayDuration={0}>
                                    <TooltipTrigger asChild>
                                        <Badge
                                            variant="outline"
                                            className="gap-1.5 text-[10px] h-5 px-2 bg-orange-500/5 text-orange-600 border-orange-500/20 cursor-help hover:bg-orange-500/10 hover:border-orange-500/30 transition-all shadow-[0_0_10px_rgba(249,115,22,0.05)]"
                                        >
                                            <span className="font-bold">
                                                {streamer.consecutive_error_count}
                                            </span>
                                            <AlertTriangle className="h-3 w-3" />
                                        </Badge>
                                    </TooltipTrigger>
                                    <TooltipContent
                                        side="bottom"
                                        className="p-0 border-border/50 shadow-2xl bg-background/95 backdrop-blur-xl overflow-hidden ring-1 ring-white/5"
                                    >
                                        <StatusInfoTooltip
                                            theme="orange"
                                            icon={<AlertTriangle className="h-3.5 w-3.5" />}
                                            title={<Trans>Connection Instability</Trans>}
                                            subtitle={
                                                <Trans>
                                                    Stream monitoring encountered issues
                                                </Trans>
                                            }
                                        >
                                            <div className="flex items-center justify-between text-xs p-2 rounded-md bg-muted/30 border border-border/40">
                                                <span className="text-muted-foreground font-medium">
                                                    <Trans>Consecutive Failures</Trans>
                                                </span>
                                                <Badge
                                                    variant="secondary"
                                                    className="font-mono font-bold text-[10px] bg-orange-500/10 text-orange-700 border-orange-500/20 dark:text-orange-400"
                                                >
                                                    {streamer.consecutive_error_count}
                                                </Badge>
                                            </div>

                                            <div className="space-y-2">
                                                <p className="text-[10px] font-medium text-muted-foreground uppercase tracking-wider flex items-center gap-1.5 ml-0.5">
                                                    <Activity className="h-3 w-3 opacity-70" />
                                                    <Trans>Last Error Log</Trans>
                                                </p>
                                                <div className="text-xs bg-muted/40 text-muted-foreground/90 p-2.5 rounded-md font-mono break-all border border-border/40 shadow-sm leading-relaxed max-h-[120px] overflow-y-auto">
                                                    {streamer.last_error || (
                                                        <span className="italic text-muted-foreground/60">
                                                            <Trans>
                                                                No detailed error message
                                                                available
                                                            </Trans>
                                                        </span>
                                                    )}
                                                </div>
                                            </div>
                                        </StatusInfoTooltip>
                                    </TooltipContent>
                                </Tooltip>
                            </TooltipProvider>
                        )}

                    <TooltipProvider>
                        <Tooltip>
                            <TooltipTrigger asChild>
                                <a
                                    href={streamer.url}
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    className="ml-auto opacity-0 group-hover:opacity-100 transition-opacity p-1 hover:bg-muted rounded-full"
                                >
                                    <ExternalLink className="h-3 w-3 hover:text-primary transition-colors" />
                                </a>
                            </TooltipTrigger>
                            <TooltipContent>
                                <p className="max-w-[300px] break-all font-mono text-xs">
                                    {streamer.url}
                                </p>
                            </TooltipContent>
                        </Tooltip>
                    </TooltipProvider>
                </div>
            </div>
        </div>
    );
};
