import { memo } from 'react';
import { StreamerSchema } from '../../api/schemas';
import { z } from 'zod';
import { CardHeader } from '../ui/card';
import { cn } from '../../lib/utils';
import { useShallow } from 'zustand/react/shallow';
import { ProgressIndicator } from './progress-indicator';
import { StatusBadge } from './card/stream-status-badge';
import { useStreamerStatus } from './card/use-streamer-status';
import { StreamActionsMenu } from './card/stream-actions-menu';
import { StreamAvatarInfo } from './card/stream-avatar-info';
import { DashboardCard } from '../dashboard/dashboard-card';
import { useDownloadStore } from '@/store/downloads';
import { Check } from 'lucide-react';

interface StreamerCardProps {
  streamer: z.infer<typeof StreamerSchema>;
  onDelete: (id: string) => void;
  onToggle: (id: string, enabled: boolean) => void;
  onCheck: (id: string) => void;
  selectionMode?: boolean;
  isSelected?: boolean;
  onSelectionChange?: (id: string, selected: boolean) => void;
}

export const StreamerCard = memo(
  ({
    streamer,
    onDelete,
    onToggle,
    onCheck,
    selectionMode = false,
    isSelected = false,
    onSelectionChange,
  }: StreamerCardProps) => {
    // Query downloads for this streamer
    const downloads = useDownloadStore(
      useShallow((state) => state.getDownloadsByStreamer(streamer.id)),
    );
    const activeDownload = downloads?.[0]; // Show first active download

    // Surface "queued waiting for slot" state when the streamer is
    // live but no active download has started yet. Cleared by the
    // store on DownloadStarted/terminal events.
    const queuedEntry = useDownloadStore(
      useShallow((state) => state.getQueuedForStreamer(streamer.id)),
    );

    const status = useStreamerStatus(streamer, activeDownload, queuedEntry);

    const toggleSelection = () => {
      if (selectionMode) {
        onSelectionChange?.(streamer.id, !isSelected);
      }
    };

    return (
      <DashboardCard
        className={cn(
          'flex flex-col h-full',
          selectionMode &&
            'cursor-pointer select-none [&_a]:pointer-events-none',
          isSelected && 'border-primary/50 ring-2 ring-primary',
          !streamer.enabled
            ? 'opacity-60 grayscale-[0.8] hover:grayscale-0 hover:opacity-100'
            : '',
        )}
        role={selectionMode ? 'checkbox' : undefined}
        aria-checked={selectionMode ? isSelected : undefined}
        tabIndex={selectionMode ? 0 : undefined}
        onClick={toggleSelection}
        onKeyDown={(event) => {
          if (selectionMode && (event.key === 'Enter' || event.key === ' ')) {
            event.preventDefault();
            toggleSelection();
          }
        }}
      >
        {selectionMode && (
          <div
            className={cn(
              'absolute right-3 top-3 z-20 flex h-6 w-6 items-center justify-center rounded-full border-2 shadow-sm transition-colors',
              isSelected
                ? 'border-primary bg-primary text-primary-foreground'
                : 'border-border bg-background/90 text-transparent',
            )}
          >
            <Check className="h-3.5 w-3.5" />
          </div>
        )}
        <CardHeader className="px-5 py-2">
          <div className="flex justify-between items-start">
            <div className="space-y-3 w-full">
              <div className="flex items-center justify-between w-full">
                <div className="flex items-center gap-2">
                  <StatusBadge status={status} />
                </div>

                {!selectionMode && (
                  <StreamActionsMenu
                    streamer={streamer}
                    onDelete={onDelete}
                    onToggle={onToggle}
                    onCheck={onCheck}
                  />
                )}
              </div>

              <StreamAvatarInfo
                streamer={streamer}
                activeDownload={activeDownload}
              />

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
