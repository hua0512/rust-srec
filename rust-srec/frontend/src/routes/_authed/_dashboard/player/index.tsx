import { createFileRoute } from '@tanstack/react-router';
import { useState, useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { useMutation } from '@tanstack/react-query';
import {
  parseUrl,
  parseUrlBatch,
  type ParseUrlResponse,
} from '@/server/functions';
import { UrlInputForm } from '@/components/player/url-input-form';
import { PlayerCard } from '@/components/player/player-card';
import { Button } from '@/components/ui/button';
import {
  StreamInfoCard,
  type StreamOption,
} from '@/components/player/stream-info-card';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Maximize2, Minimize2, Video, Plus, Loader2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { toast } from 'sonner';
import { motion, AnimatePresence } from 'motion/react';
import { cn } from '@/lib/utils';

import { z } from 'zod';

const playerSearchSchema = z.object({
  url: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/player/')({
  validateSearch: (search) => playerSearchSchema.parse(search),
  component: PlayerPage,
});

interface PlayerInstance {
  id: string;
  currentStream: StreamOption;
  title: string;
  headers?: Record<string, string>;
  response: ParseUrlResponse;
}

function PlayerPage() {
  const [players, setPlayers] = useState<PlayerInstance[]>([]);
  const [isImmersive, setIsImmersive] = useState(false);
  const [isAddStreamOpen, setIsAddStreamOpen] = useState(false);
  const [isParsing, setIsParsing] = useState(false);

  const { url } = Route.useSearch();
  const autoPlayProcessed = useRef<string | null>(null);

  // Auto-play from URL parameter
  useEffect(() => {
    if (
      url &&
      !players.some((p) => p.title === url) &&
      autoPlayProcessed.current !== url
    ) {
      autoPlayProcessed.current = url;
      parseSingleMutation.mutate({ url });
    }
    // If url is cleared or changed empty, reset? No, keep logic simple.
  }, [url]); // Trigger when url changes

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setIsImmersive(false);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  const parseSingleMutation = useMutation({
    mutationFn: (data: { url: string; cookies?: string }) => parseUrl({ data }),
    onMutate: () => setIsParsing(true),
    onSettled: () => setIsParsing(false),
    onSuccess: (response, variables) => {
      if (players.some((p) => p.title === variables.url)) {
        toast.warning('This stream is already active');
        return;
      }

      if (response.success && response.media_info) {
        // Extract first available stream
        const firstStream = extractFirstStream(response.media_info);
        if (firstStream) {
          const newPlayer: PlayerInstance = {
            id: Date.now().toString(),
            currentStream: firstStream,
            title: variables.url,
            headers: variables.cookies
              ? { Cookie: variables.cookies }
              : undefined,
            response,
          };
          setPlayers((prev) => [...prev, newPlayer]);
          toast.success('Stream parsed successfully');
        } else {
          toast.error('No playable stream found in response');
        }
      } else {
        toast.error(response.error || 'Failed to parse URL');
      }
    },
    onError: (error) => {
      toast.error(
        `Parse error: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    },
  });

  const parseBatchMutation = useMutation({
    mutationFn: (data: { urls: string[]; cookies?: string }) => {
      const requests = data.urls.map((url) => ({
        url,
        cookies: data.cookies,
      }));
      return parseUrlBatch({ data: requests });
    },
    onMutate: () => setIsParsing(true),
    onSettled: () => setIsParsing(false),
    onSuccess: (responses, variables) => {
      const newPlayers: PlayerInstance[] = [];
      let skippedCount = 0;

      responses.forEach((response, index) => {
        const url = variables.urls[index];

        // Check for duplicates in existing players or current batch
        const isDuplicate =
          players.some((p) => p.title === url) ||
          newPlayers.some((p) => p.title === url);

        if (isDuplicate) {
          skippedCount++;
          return;
        }

        if (response.success && response.media_info) {
          const firstStream = extractFirstStream(response.media_info);
          if (firstStream) {
            newPlayers.push({
              id: `${Date.now()}-${index}`,
              currentStream: firstStream,
              title: url || `Stream ${index + 1}`,
              headers: variables.cookies
                ? { Cookie: variables.cookies }
                : undefined,
              response,
            });
          }
        }
      });

      if (newPlayers.length > 0) {
        setPlayers((prev) => [...prev, ...newPlayers]);
        toast.success(`${newPlayers.length} stream(s) added successfully`);
      }

      if (skippedCount > 0) {
        toast.warning(`Skipped ${skippedCount} duplicate stream(s)`);
      } else if (newPlayers.length === 0 && responses.length > 0) {
        // Nothing added and not just because of duplicates (e.g. all failed)
        if (skippedCount === 0) toast.error('No playable streams found');
      }
    },
    onError: (error) => {
      toast.error(
        `Batch parse error: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    },
  });

  const handleRemovePlayer = (id: string) => {
    setPlayers((prev) => prev.filter((p) => p.id !== id));
  };

  const handleStreamChange = (playerId: string, stream: StreamOption) => {
    setPlayers((prev) =>
      prev.map((p) =>
        p.id === playerId ? { ...p, currentStream: stream } : p,
      ),
    );
  };

  const isLoading = isParsing;

  const playerGrid = (
    <motion.div
      key={isImmersive ? 'immersive' : 'standard'}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ delay: 0.2 }}
      className={cn(
        isImmersive
          ? 'fixed inset-0 z-[100] bg-background/95 backdrop-blur-sm p-6 pt-20 overflow-y-auto grid gap-6 content-start'
          : 'grid gap-8',
        players.length === 1 ? 'grid-cols-1' : 'grid-cols-1 2xl:grid-cols-2',
      )}
    >
      {isImmersive && (
        <Button
          size="sm"
          variant="outline"
          className="fixed top-6 right-8 z-[200] shadow-md gap-2 ring-1 ring-foreground/10"
          onClick={() => setIsImmersive(false)}
        >
          <Minimize2 className="h-4 w-4" />
          <span className="sr-only sm:not-sr-only">
            <Trans>Exit Immersive</Trans>
          </span>
        </Button>
      )}
      {players.map((player, index) => (
        <motion.div
          key={player.id}
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ delay: index * 0.1 }}
          className="w-full"
        >
          <div className="w-full h-full min-h-[500px]">
            <PlayerCard
              url={player.currentStream.url}
              title={player.title}
              headers={{ ...player.currentStream.headers, ...player.headers }}
              streamData={
                // Find the original stream object from media_info
                player.response.media_info?.streams?.find(
                  (s: any) =>
                    s.url === player.currentStream.url ||
                    s.src === player.currentStream.url,
                ) || player.response.media_info?.streams?.[0]
              }
              onRemove={() => handleRemovePlayer(player.id)}
              settingsContent={
                <StreamInfoCard
                  mediaInfo={player.response.media_info}
                  selectedStream={player.currentStream}
                  onStreamSelect={(stream) =>
                    handleStreamChange(player.id, stream)
                  }
                  isLive={player.response.is_live}
                  variant="minimal"
                />
              }
            />
          </div>
        </motion.div>
      ))}
    </motion.div>
  );

  return (
    <div className="relative min-h-screen overflow-x-hidden selection:bg-primary/20">
      {/* Background Decoration */}
      <div className="fixed inset-0 pointer-events-none">
        <div className="absolute top-0 right-0 -mt-20 -mr-20 w-[500px] h-[500px] bg-primary/5 rounded-full blur-[120px]" />
        <div className="absolute bottom-0 left-0 -mb-40 -ml-20 w-[600px] h-[600px] bg-blue-500/5 rounded-full blur-[120px]" />
      </div>

      <div className="relative z-10 full-width px-6 py-8 pb-32">
        {/* Header */}
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          className="mb-12"
        >
          <div className="flex items-center justify-between mb-2">
            <div className="flex items-center gap-4">
              <div className="flex items-center justify-center w-16 h-16 rounded-2xl shadow-xl ring-1 ring-white/10 backdrop-blur-md bg-gradient-to-br from-primary/20 to-primary/5">
                <Video className="h-8 w-8 text-primary" />
              </div>
              <div>
                <h1 className="text-3xl font-bold tracking-tight">
                  <Trans>Stream Player</Trans>
                </h1>
                <p className="text-muted-foreground">
                  <Trans>Parse and play live streams</Trans>
                </p>
              </div>
            </div>
            {players.length > 0 && (
              <Button
                variant="outline"
                size="sm"
                className="gap-2 hidden sm:flex"
                onClick={() => setIsImmersive(!isImmersive)}
              >
                {isImmersive ? (
                  <>
                    <Minimize2 className="h-4 w-4" />
                    <Trans>Exit Immersive</Trans>
                  </>
                ) : (
                  <>
                    <Maximize2 className="h-4 w-4" />
                    <Trans>Immersive Mode</Trans>
                  </>
                )}
              </Button>
            )}
          </div>
        </motion.div>

        {/* Players Grid */}
        {players.length > 0 ? (
          isImmersive ? (
            createPortal(playerGrid, document.body)
          ) : (
            playerGrid
          )
        ) : isLoading ? (
          <motion.div
            key="loading"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            className="flex flex-col items-center justify-center min-h-[60vh] text-center space-y-6"
          >
            <div className="relative">
              <div className="absolute inset-0 bg-primary/20 blur-3xl rounded-full" />
              <div className="relative p-8 rounded-full bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border border-white/10 shadow-2xl ring-1 ring-white/5">
                <Loader2 className="h-20 w-20 text-primary/80 animate-spin" />
              </div>
            </div>
            <div className="space-y-2">
              <h3 className="text-3xl font-bold tracking-tight text-foreground/90">
                <Trans>Parsing Stream...</Trans>
              </h3>
              <p className="text-muted-foreground text-lg max-w-md mx-auto leading-relaxed">
                <Trans>Please wait while we resolve the stream URL.</Trans>
              </p>
            </div>
          </motion.div>
        ) : (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ delay: 0.2 }}
            className="flex flex-col items-center justify-center min-h-[60vh] text-center space-y-6"
          >
            <div className="relative">
              <div className="absolute inset-0 bg-primary/20 blur-3xl rounded-full" />
              <div className="relative p-8 rounded-full bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border border-white/10 shadow-2xl ring-1 ring-white/5">
                <Video className="h-20 w-20 text-primary/80" />
              </div>
            </div>
            <div className="space-y-2">
              <h3 className="text-3xl font-bold tracking-tight text-foreground/90">
                <Trans>Ready to Watch</Trans>
              </h3>
              <p className="text-muted-foreground text-lg max-w-md mx-auto leading-relaxed">
                <Trans>Add a stream to start your viewing session.</Trans>
              </p>
            </div>
            <Button
              size="lg"
              className="mt-8 gap-2 rounded-full h-12 px-8 text-base shadow-xl shadow-primary/20 hover:shadow-primary/30 transition-all duration-300 hover:scale-105"
              onClick={() => setIsAddStreamOpen(true)}
            >
              <Plus className="h-5 w-5" />
              <Trans>Add Stream</Trans>
            </Button>
          </motion.div>
        )}

        {/* FAB */}
        <AnimatePresence>
          {!isImmersive && players.length > 0 && (
            <motion.div
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              whileHover={{ scale: 1.1 }}
              whileTap={{ scale: 0.9 }}
              className="fixed bottom-8 right-8 z-50"
            >
              <Button
                onClick={() => setIsAddStreamOpen(true)}
                size="icon"
                className="h-16 w-16 rounded-full shadow-2xl bg-primary hover:bg-primary/90 text-primary-foreground flex items-center justify-center p-0 ring-4 ring-background/50 backdrop-blur-sm"
              >
                <Plus className="h-8 w-8" />
              </Button>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Add Stream Dialog */}
        <Dialog open={isAddStreamOpen} onOpenChange={setIsAddStreamOpen}>
          <DialogContent className="sm:max-w-md border-border/50 bg-background/80 backdrop-blur-xl duration-200">
            <DialogHeader>
              <DialogTitle>
                <Trans>Add Stream</Trans>
              </DialogTitle>
              <DialogDescription>
                <Trans>Enter a URL to parse and play a stream</Trans>
              </DialogDescription>
            </DialogHeader>
            <div className="mt-4">
              <UrlInputForm
                onSubmitSingle={(data) => {
                  parseSingleMutation.mutate(data);
                  setIsAddStreamOpen(false);
                }}
                onSubmitBatch={(data) => {
                  parseBatchMutation.mutate(data);
                  setIsAddStreamOpen(false);
                }}
                isLoading={isLoading}
              />
            </div>
          </DialogContent>
        </Dialog>
      </div>
    </div>
  );
}

// Helper function to extract first available stream from media_info
function extractFirstStream(mediaInfo: any): StreamOption | null {
  if (!mediaInfo) return null;

  // Handle array of streams
  if (Array.isArray(mediaInfo.streams) && mediaInfo.streams.length > 0) {
    const stream = mediaInfo.streams[0];
    return {
      url: stream.url || stream.src || '',
      quality: stream.quality || stream.resolution || 'default',
      cdn: stream.cdn || stream.server,
      format: stream.format || detectFormat(stream.url),
      bitrate: stream.bitrate || stream.bandwidth,
      headers: { ...mediaInfo.headers, ...stream.headers },
    };
  }

  // Handle single stream object
  if (mediaInfo.url) {
    return {
      url: mediaInfo.url,
      quality: mediaInfo.quality || 'default',
      cdn: mediaInfo.cdn,
      format: mediaInfo.format || detectFormat(mediaInfo.url),
      bitrate: mediaInfo.bitrate,
      headers: mediaInfo.headers || {},
    };
  }

  // Handle string URL
  if (typeof mediaInfo === 'string') {
    return {
      url: mediaInfo,
      quality: 'default',
      format: detectFormat(mediaInfo),
    };
  }

  console.log('Unable to extract stream from media_info:', mediaInfo);
  return null;
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
