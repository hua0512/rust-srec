import { memo } from 'react';
import { StreamerSchema } from '../../api/schemas';
import { z } from 'zod';
import { CardHeader } from '../ui/card';
import { cn } from '../../lib/utils';
import { useDownloadStore } from '../../store/downloads';
import { useShallow } from 'zustand/react/shallow';
import { useStore } from '@/hooks/use-store';
import { ProgressIndicator } from './progress-indicator';
import { StatusBadge } from './card/stream-status-badge';
import { useStreamerStatus } from './card/use-streamer-status';
import { StreamActionsMenu } from './card/stream-actions-menu';
import { StreamAvatarInfo } from './card/stream-avatar-info';

import { DashboardCard } from '../dashboard/dashboard-card';

interface StreamerCardProps {
  streamer: z.infer<typeof StreamerSchema>;
  onDelete: (id: string) => void;
  onToggle: (id: string, enabled: boolean) => void;
  onCheck: (id: string) => void;
}

export const StreamerCard = memo(
  ({ streamer, onDelete, onToggle, onCheck }: StreamerCardProps) => {
    // Query downloads for this streamer - using useStore for hydration safety
    const downloads = useStore(
      useDownloadStore,
      useShallow((state) => state.getDownloadsByStreamer(streamer.id)),
    );
    const activeDownload = downloads?.[0]; // Show first active download

    const status = useStreamerStatus(streamer);

    return (
      <DashboardCard
        className={cn(
          'flex flex-col h-full',
          !streamer.enabled
            ? 'opacity-60 grayscale-[0.8] hover:grayscale-0 hover:opacity-100'
            : '',
        )}
      >
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
      </DashboardCard>
    );
  },
);

StreamerCard.displayName = 'StreamerCard';
