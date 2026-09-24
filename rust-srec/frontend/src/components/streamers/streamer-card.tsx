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
import { useUploadStore } from '@/store/uploads';
import { Check } from 'lucide-react';
import { UploadIndicator } from './card/upload-indicator';
import { hasStrongRecoverySignal } from './card/recovery-state';

interface StreamerCardProps {
  streamer: z.infer<typeof StreamerSchema>;
  onDelete: (id: string) => void;
  onToggle: (id: string, enabled: boolean) => void;
  selectionMode?: boolean;
  isSelected?: boolean;
  onSelectionChange?: (id: string, selected: boolean) => void;
}

export const StreamerCard = memo(
  ({
    streamer,
    onDelete,
    onToggle,
    selectionMode = false,
    isSelected = false,
    onSelectionChange,
  }: StreamerCardProps) => {
    // The card reads only what changes its layout: which download is shown
    // and whether it has made enough progress to count as recovered. The
    // figures that change on every progress tick are read by
    // ProgressIndicator, so a tick re-renders that alone.
    const activeDownloadId = useDownloadStore(
      (state) => state.getFirstDownloadByStreamer(streamer.id)?.downloadId,
    );
    const hasRecoverySignal = useDownloadStore((state) =>
      hasStrongRecoverySignal(state.getFirstDownloadByStreamer(streamer.id)),
    );

    // Surface "queued waiting for slot" state when the streamer is
    // live but no active download has started yet. Cleared by the
    // store on DownloadStarted/terminal events.
    const queuedEntry = useDownloadStore(
      useShallow((state) => state.getQueuedForStreamer(streamer.id)),
    );

    // Live upload jobs for this streamer, pushed over the WS into the
    // uploads store (separate from the downloads store; see store/uploads.ts).
    const hasActiveUploads = useUploadStore(
      (state) => state.getActiveUploadsByStreamer(streamer.id).length > 0,
    );

    const status = useStreamerStatus(
      streamer,
      activeDownloadId !== undefined,
      hasRecoverySignal,
      queuedEntry,
    );

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
                  {hasActiveUploads && (
                    <UploadIndicator streamerId={streamer.id} />
                  )}
                </div>

                {!selectionMode && (
                  <StreamActionsMenu
                    streamer={streamer}
                    onDelete={onDelete}
                    onToggle={onToggle}
                  />
                )}
              </div>

              <StreamAvatarInfo
                streamer={streamer}
                hasRecoverySignal={hasRecoverySignal}
              />

              {/* Download progress indicator */}
              {activeDownloadId !== undefined && (
                <ProgressIndicator downloadId={activeDownloadId} />
              )}
            </div>
          </div>
        </CardHeader>
      </DashboardCard>
    );
  },
);

StreamerCard.displayName = 'StreamerCard';
