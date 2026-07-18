import type { ReactNode } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { AlertCircle, Loader2, RefreshCcw, Settings2, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { usePlayerPlayback } from './use-player-playback';

export interface PlayerCardProps {
  url: string;
  title?: string;
  headers?: Record<string, string>;
  streamData?: unknown;
  onRemove?: () => void;
  className?: string;
  contentClassName?: string;
  settingsContent?: ReactNode;
  muted?: boolean;
  volume?: number;
  onVolumeChange?: (volume: number) => void;
  onMuteChange?: (muted: boolean) => void;
  defaultWebFullscreen?: boolean;
  mediaType?: string;
  isLive?: boolean;
  mediaDurationSecs?: number | null;
  mediaFileSizeBytes?: number;
}

export function PlayerCard({
  url,
  title,
  headers,
  streamData,
  onRemove,
  className,
  settingsContent,
  contentClassName,
  muted = false,
  volume = 0.5,
  onVolumeChange,
  onMuteChange,
  defaultWebFullscreen = false,
  mediaType,
  isLive = false,
  mediaDurationSecs,
  mediaFileSizeBytes,
}: PlayerCardProps) {
  const { containerRef, error, loading, reload } = usePlayerPlayback({
    url,
    headers,
    title,
    streamData,
    muted,
    volume,
    onVolumeChange,
    onMuteChange,
    defaultWebFullscreen,
    mediaType,
    isLive,
    mediaDurationSecs,
    mediaFileSizeBytes,
  });

  return (
    <Card
      className={cn(
        'relative h-full flex flex-col transition-all duration-500 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20',
        className,
      )}
    >
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center justify-between gap-4 pb-2 space-y-0 z-10">
        <div className="flex items-center gap-3 min-w-0">
          <div className="p-2 rounded-xl bg-primary/10 ring-1 ring-inset ring-primary/20 transition-transform duration-500 group-hover:scale-110">
            <div className="h-4 w-4 bg-primary rounded-full animate-pulse" />
          </div>
          <div className="flex flex-col min-w-0">
            <CardTitle className="text-sm font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
              {title || 'Video Player'}
            </CardTitle>
          </div>
        </div>

        <div className="flex items-center gap-2">
          {loading && (
            <Badge
              variant="secondary"
              className="gap-1 bg-background/50 backdrop-blur"
            >
              <Loader2 className="h-3 w-3 animate-spin" />
              Loading
            </Badge>
          )}
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
            onClick={reload}
            title="Reload Player"
          >
            <RefreshCcw className="h-4 w-4" />
          </Button>
          {settingsContent && (
            <Popover>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
                >
                  <Settings2 className="h-4 w-4" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="end"
                className="w-[320px] p-4 backdrop-blur-xl bg-background/80 border-border/40 text-foreground z-[200]"
              >
                {settingsContent}
              </PopoverContent>
            </Popover>
          )}
          {onRemove && (
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 text-muted-foreground/40 hover:text-destructive hover:bg-destructive/10 transition-colors rounded-full"
              onClick={onRemove}
            >
              <X className="h-4 w-4" />
            </Button>
          )}
        </div>
      </CardHeader>

      <CardContent
        className={cn(
          'relative p-0 flex-1 min-h-[500px] bg-black/50 group-hover:bg-black/40 transition-colors rounded-b-xl overflow-hidden',
          contentClassName,
        )}
      >
        <div ref={containerRef} className="w-full h-full absolute inset-0" />
        {loading && !error && (
          <div className="pointer-events-none absolute inset-0 z-[5] flex items-center justify-center bg-black/20">
            <Loader2 className="h-8 w-8 animate-spin text-white/80 drop-shadow" />
          </div>
        )}
        {error && (
          <div className="absolute inset-0 z-10 flex flex-col items-center justify-center h-full text-center space-y-3 p-8 bg-black/90">
            <div className="p-3 rounded-full bg-destructive/10 text-destructive mb-2">
              <AlertCircle className="h-8 w-8" />
            </div>
            <p className="text-sm font-medium text-destructive">
              Playback Error
            </p>
            <p className="text-xs text-muted-foreground max-w-[250px]">
              {error}
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
