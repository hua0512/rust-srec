import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
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

export interface StreamOption {
  url: string;
  quality?: string;
  cdn?: string;
  format?: string;
  bitrate?: number;
  headers?: Record<string, string>;
  extras?: Record<string, string>;
}

export interface StreamInfoCardProps {
  mediaInfo: any;
  selectedStream: StreamOption | null;
  onStreamSelect: (stream: StreamOption) => void;
  isLive?: boolean;
  variant?: 'card' | 'minimal';
}

export function StreamInfoCard({
  mediaInfo,
  selectedStream,
  onStreamSelect,
  isLive = false,
  variant = 'card',
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

  const content = (
    <>
      {/* Header for minimal mode */}
      {variant === 'minimal' && (
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
      )}

      {variant === 'card' && (
        <CardHeader className="pb-4 relative z-10">
          <div className="flex items-center justify-between">
            <CardTitle className="text-sm font-medium flex items-center gap-2.5">
              <div className="p-2 rounded-lg bg-primary/10 text-primary ring-1 ring-inset ring-primary/20">
                <Video className="h-4 w-4" />
              </div>
              <span className="text-foreground/90">
                <Trans>Stream Options</Trans>
              </span>
            </CardTitle>
            {isLive && (
              <Badge variant="destructive" className="gap-1.5 shadow-sm">
                <Radio className="h-3 w-3 animate-pulse" />
                <Trans>LIVE</Trans>
              </Badge>
            )}
          </div>
        </CardHeader>
      )}

      <CardContent
        className={cn(
          'space-y-6 relative z-10',
          variant === 'minimal' ? 'p-1' : '',
        )}
      >
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
    </>
  );

  if (variant === 'minimal') {
    return <div className="relative">{content}</div>;
  }

  return (
    <Card className="relative h-full flex flex-col transition-all duration-500 hover:shadow-xl hover:shadow-primary/5 group overflow-hidden bg-gradient-to-br from-background/90 to-background/50 backdrop-blur-xl border-border/40 hover:border-primary/20">
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/30 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />
      {content}
    </Card>
  );
}

// Helper function to extract all stream options from media_info
function extractStreams(mediaInfo: any): StreamOption[] {
  const streams: StreamOption[] = [];

  if (!mediaInfo) return streams;

  // Handle different possible structures
  if (Array.isArray(mediaInfo.streams)) {
    mediaInfo.streams.forEach((stream: any) => {
      const extras = stringifyValues({ ...mediaInfo.extras, ...stream.extras });
      streams.push({
        url: stream.url || stream.src || '',
        quality: stream.quality || stream.resolution || 'unknown',
        cdn: stream.cdn || stream.server || extras.cdn,
        format: stream.format || detectFormat(stream.url),
        bitrate: stream.bitrate || stream.bandwidth,
        headers: { ...mediaInfo.headers, ...stream.headers },
        extras,
      });
    });
  } else if (mediaInfo.url) {
    // Single stream
    const extras = stringifyValues(mediaInfo.extras || {});
    streams.push({
      url: mediaInfo.url,
      quality: mediaInfo.quality || 'default',
      cdn: mediaInfo.cdn || extras.cdn,
      format: mediaInfo.format || detectFormat(mediaInfo.url),
      bitrate: mediaInfo.bitrate,
      headers: mediaInfo.headers || {},
      extras,
    });
  }

  return streams.filter((s) => s.url);
}

// Detect format from URL
function detectFormat(url: string): string {
  if (!url) return 'unknown';
  if (url.includes('.m3u8')) return 'hls';
  if (url.includes('.flv')) return 'flv';
  if (url.includes('.ts')) return 'mpegts';
  if (url.includes('.mp4')) return 'mp4';
  return 'unknown';
}

function stringifyValues(obj: Record<string, any>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key in obj) {
    if (obj[key] !== undefined && obj[key] !== null) {
      result[key] = String(obj[key]);
    }
  }
  return result;
}
