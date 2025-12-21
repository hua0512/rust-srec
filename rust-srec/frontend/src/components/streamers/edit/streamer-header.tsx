import { Link } from '@tanstack/react-router';
import { ArrowLeft } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';

interface StreamerHeaderProps {
  streamer: any;
  isRecording: boolean;
  isLive: boolean;
  platform: string;
}

export function StreamerHeader({
  streamer,
  isRecording,
  isLive,
  platform,
}: StreamerHeaderProps) {
  const statusColor = isRecording
    ? 'text-red-500'
    : isLive
      ? 'text-green-500'
      : 'text-muted-foreground';
  const statusBg = isRecording
    ? 'bg-red-500/10 border-red-500/20'
    : isLive
      ? 'bg-green-500/10 border-green-500/20'
      : 'bg-muted/50 border-transparent';

  return (
    <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
      <div className="flex items-start gap-3 md:gap-4 overflow-hidden">
        <Button
          variant="ghost"
          size="icon"
          className="h-10 w-10 shrink-0 rounded-full bg-background border shadow-sm hover:bg-muted"
          asChild
        >
          <Link to="/streamers">
            <ArrowLeft className="h-5 w-5 text-muted-foreground" />
          </Link>
        </Button>

        <div className="space-y-1 min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2 md:gap-3">
            <h1 className="text-2xl md:text-3xl font-bold tracking-tight text-foreground truncate">
              {streamer?.name}
            </h1>
            <Badge
              variant="outline"
              className={cn(
                'capitalize px-2.5 py-0.5 text-xs font-semibold rounded-full border transition-colors shrink-0',
                statusBg,
                statusColor,
              )}
            >
              {isRecording ? (
                <Trans>Recording</Trans>
              ) : isLive ? (
                <Trans>Live</Trans>
              ) : (
                <Trans>Offline</Trans>
              )}
            </Badge>
          </div>
          <div className="flex flex-wrap items-center text-sm text-muted-foreground gap-2">
            <span className="capitalize shrink-0">
              {platform.toLowerCase()}
            </span>
            <span className="hidden sm:inline">•</span>
            <span className="font-mono text-xs opacity-70 hidden sm:inline">
              ID: {streamer?.id}
            </span>
            <div className="flex items-center gap-1 text-xs font-mono bg-muted/30 px-2 py-0.5 rounded-md max-w-full sm:ml-2 overflow-hidden">
              <span className="truncate">{streamer?.url}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
