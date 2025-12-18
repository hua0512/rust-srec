import { StreamerSchema } from '../../api/schemas';
import { z } from 'zod';
import { Card, CardHeader } from '../ui/card';
import { cn } from '../../lib/utils';
import { useDownloadStore } from '../../store/downloads';
import { useShallow } from 'zustand/react/shallow';
import { ProgressIndicator } from './progress-indicator';
import { StatusBadge } from './card/stream-status-badge';
import { useStreamerStatus } from './card/use-streamer-status';
import { StreamActionsMenu } from './card/stream-actions-menu';
import { StreamAvatarInfo } from './card/stream-avatar-info';

interface StreamerCardProps {
  streamer: z.infer<typeof StreamerSchema>;
  onDelete: (id: string) => void;
  onToggle: (id: string, enabled: boolean) => void;
  onCheck: (id: string) => void;
}

export function StreamerCard({
  streamer,
  onDelete,
  onToggle,
  onCheck,
}: StreamerCardProps) {
  // Query downloads for this streamer
  const downloads = useDownloadStore(
    useShallow((state) => state.getDownloadsByStreamer(streamer.id)),
  );
  const activeDownload = downloads[0]; // Show first active download

  const status = useStreamerStatus(streamer);

  return (
    <Card
      className={cn(
        'group overflow-hidden transition-all duration-300 hover:shadow-md dark:hover:shadow-2xl dark:hover:shadow-black/5 hover:-translate-y-1 h-full flex flex-col bg-white/60 dark:bg-card/40 backdrop-blur-xl border-black/5 dark:border-white/5 shadow-sm relative',
        !streamer.enabled
          ? 'opacity-60 grayscale-[0.8] hover:grayscale-0 hover:opacity-100'
          : '',
      )}
    >
      <div className="absolute inset-x-0 top-0 h-1 bg-gradient-to-r from-transparent via-primary/20 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-300" />
      <CardHeader className="px-5 py-4">
        <div className="flex justify-between items-start">
          <div className="space-y-3 w-full">
            <div className="flex items-center justify-between w-full">
              <div className="flex items-center gap-2">
                <StatusBadge status={status} />
              </div>

              <StreamActionsMenu
                streamer={streamer}
                onDelete={onDelete}
                onToggle={onToggle}
                onCheck={onCheck}
              />
            </div>

            <StreamAvatarInfo streamer={streamer} />

            {/* Download progress indicator */}
            {activeDownload && (
              <ProgressIndicator progress={activeDownload} compact />
            )}
          </div>
        </div>
      </CardHeader>
    </Card>
  );
}
