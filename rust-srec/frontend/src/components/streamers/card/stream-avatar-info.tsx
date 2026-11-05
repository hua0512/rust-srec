import { StreamerAvatar } from '@/components/shared/streamer-avatar';
import { CardTitle } from '@/components/ui/card';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { Badge } from '@/components/ui/badge';
import { ExternalLink, AlertTriangle, Activity } from 'lucide-react';
import { cn, getPlatformFromUrl } from '@/lib/utils';
import { formatPlatformName } from '@/lib/format';
import { Trans } from '@lingui/react/macro';
import { z } from 'zod';
import { StreamerSchema } from '@/api/schemas';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';
import {
  getPlatformColor,
  getPlatformIcon,
} from '@/components/pipeline/constants';

interface StreamAvatarInfoProps {
  streamer: z.infer<typeof StreamerSchema>;
  /** Shows the consecutive-failure badge while the streamer has any. */
  showErrorCount: boolean;
}

export const StreamAvatarInfo = ({
  streamer,
  showErrorCount,
}: StreamAvatarInfoProps) => {
  const platform = getPlatformFromUrl(streamer.url);
  const PlatformIcon = getPlatformIcon(platform);

  return (
    <div className="flex items-center gap-3">
      <StreamerAvatar
        src={streamer.avatar_url}
        name={streamer.name}
        className="h-10 w-10 border border-border/60 shadow-sm transition-transform group-hover:scale-105"
        fallbackClassName="text-xs"
      />
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
              'flex items-center gap-1 px-1.5 py-0.5 rounded-md border',
              platform === 'other' ? 'bg-muted/40' : getPlatformColor(platform),
            )}
          >
            <PlatformIcon className="h-3 w-3" />
            <span className="font-medium">{formatPlatformName(platform)}</span>
          </div>

          {/* Consecutive errors */}
          {streamer.consecutive_error_count > 0 && showErrorCount && (
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
                  className="p-0 border-none shadow-none bg-transparent overflow-hidden"
                >
                  <StatusInfoTooltip
                    theme="orange"
                    icon={<AlertTriangle className="h-3.5 w-3.5" />}
                    title={<Trans>Connection Instability</Trans>}
                    subtitle={
                      <Trans>Stream monitoring encountered issues</Trans>
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
                        <Activity className="h-3 w-3 text-[var(--tooltip-theme-color)]" />
                        <Trans>Last Error Log</Trans>
                      </p>
                      <div className="text-xs bg-muted/40 text-muted-foreground/90 p-2.5 rounded-md font-mono break-all border border-border/40 shadow-sm leading-relaxed max-h-[120px] overflow-y-auto">
                        {streamer.last_error || (
                          <span className="italic text-muted-foreground/60">
                            <Trans>No detailed error message available</Trans>
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
                  className="ml-auto opacity-100 md:opacity-0 md:group-hover:opacity-100 transition-opacity p-1 hover:bg-muted rounded-full"
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
