import { CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Separator } from '@/components/ui/separator';
import { CheckCircle2, Radio, Server, Video } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';

import { extractStreams, type StreamOption } from './stream-source';
export type { StreamOption } from './stream-source';

export interface StreamInfoCardProps {
  mediaInfo: any;
  selectedStream: StreamOption | null;
  onStreamSelect: (stream: StreamOption) => void;
  isLive?: boolean;
}

export function StreamInfoCard({
  mediaInfo,
  selectedStream,
  onStreamSelect,
  isLive = false,
}: StreamInfoCardProps) {
  const streams = extractStreams(mediaInfo);

  const getSourceKey = (stream: StreamOption | null | undefined) => {
    if (!stream) return 'default';
    return stream.cdn && stream.cdn !== stream.format
      ? `${stream.cdn} (${stream.format})`
      : stream.cdn || stream.format || 'default';
  };

  // Group streams by source/CDN first
  const sourceGroups = streams.reduce(
    (acc, stream) => {
      const source = getSourceKey(stream);
      if (!acc[source]) {
        acc[source] = [];
      }
      acc[source].push(stream);
      return acc;
    },
    {} as Record<string, StreamOption[]>,
  );

  const sources = Object.keys(sourceGroups);
  const selectedSource = getSourceKey(selectedStream);
  const streamsForSource = sourceGroups[selectedSource] || [];

  // Get unique qualities for the selected source
  const qualitiesForSource = [
    ...new Set(streamsForSource.map((s) => s.quality || 'unknown')),
  ];

  return (
    <div className="relative">
      <div className="flex items-center justify-between mb-4 px-1">
        <div className="flex items-center gap-2.5">
          <div className="p-1.5 rounded-lg bg-primary/10 text-primary ring-1 ring-inset ring-primary/20">
            <Video className="h-3.5 w-3.5" />
          </div>
          <span className="text-sm font-medium text-foreground/90">
            <Trans>Stream Options</Trans>
          </span>
        </div>
        {isLive && (
          <Badge variant="destructive" className="gap-1.5 shadow-sm h-6">
            <Radio className="h-3 w-3 animate-pulse" />
            <Trans>LIVE</Trans>
          </Badge>
        )}
      </div>

      <CardContent className="space-y-6 relative z-10 p-1">
        {/* Source/CDN Selection - Primary selector */}
        {sources.length >= 1 && (
          <div className="space-y-3">
            <label className="text-[10px] font-bold text-muted-foreground/70 uppercase tracking-wider flex items-center gap-1.5">
              <div className="h-1 w-1 rounded-full bg-primary/50" />
              <Trans>Source</Trans>
            </label>
            <Select
              value={selectedSource}
              onValueChange={(source) => {
                const streamsForNewSource = sourceGroups[source] || [];
                if (streamsForNewSource.length > 0) {
                  onStreamSelect(streamsForNewSource[0]);
                }
              }}
            >
              <SelectTrigger className="w-full bg-background/50 backdrop-blur-sm border-border/60 hover:border-primary/30 transition-colors focus:ring-primary/20 h-9">
                <SelectValue />
              </SelectTrigger>
              <SelectContent className="z-[300]">
                {sources.map((source) => (
                  <SelectItem key={source} value={source}>
                    <div className="flex items-center gap-2">
                      <Badge
                        variant="outline"
                        className="text-[10px] h-5 px-1.5 bg-background/50"
                      >
                        {source.toUpperCase()}
                      </Badge>
                      <span className="text-xs text-muted-foreground">
                        ({sourceGroups[source]?.length || 0} streams)
                      </span>
                    </div>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}

        {qualitiesForSource.length > 0 && (
          <Separator className="bg-border/40" />
        )}

        {/* Quality Selection - Based on selected source */}
        {qualitiesForSource.length > 1 && (
          <div className="space-y-3">
            <label className="text-[10px] font-bold text-muted-foreground/70 uppercase tracking-wider flex items-center gap-1.5">
              <div className="h-1 w-1 rounded-full bg-primary/50" />
              <Trans>Quality</Trans>
            </label>
            <div className="grid grid-cols-2 gap-2">
              {qualitiesForSource.map((quality: string) => {
                const isSelected = quality === selectedStream?.quality;
                const streamForQuality = streamsForSource.find(
                  (s) => s.quality === quality,
                );
                return (
                  <Button
                    key={quality}
                    variant={isSelected ? 'default' : 'outline'}
                    size="sm"
                    className={cn(
                      'justify-start h-9 transition-all duration-300',
                      isSelected &&
                        'shadow-md shadow-primary/20 ring-1 ring-primary/20',
                      !isSelected &&
                        'hover:bg-primary/5 hover:text-primary hover:border-primary/20',
                    )}
                    onClick={() =>
                      streamForQuality && onStreamSelect(streamForQuality)
                    }
                  >
                    {isSelected && (
                      <CheckCircle2 className="mr-2 h-3.5 w-3.5" />
                    )}
                    <span className="truncate">{quality}</span>
                  </Button>
                );
              })}
            </div>
          </div>
        )}

        {/* Current Selection Info */}
        {selectedStream && (
          <div className="rounded-lg bg-muted/30 p-3 border border-border/40">
            <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
              {selectedStream.format && (
                <Badge
                  variant="secondary"
                  className="text-[10px] h-5 px-2 font-medium bg-secondary/50"
                >
                  {selectedStream.format.toUpperCase()}
                </Badge>
              )}
              {selectedStream.cdn && (
                <div className="flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-background/50 border border-border/40">
                  <Server className="h-3 w-3 text-primary/70" />
                  <span className="font-medium text-foreground/80">
                    {selectedStream.cdn}
                  </span>
                </div>
              )}
              {selectedStream.bitrate && (
                <div className="flex items-center gap-1.5 px-2 py-0.5">
                  <div className="h-1.5 w-1.5 rounded-full bg-green-500/70" />
                  <span className="font-mono text-foreground/70">
                    {(selectedStream.bitrate / 1000).toFixed(0)} kbps
                  </span>
                </div>
              )}
            </div>
          </div>
        )}
      </CardContent>
    </div>
  );
}
