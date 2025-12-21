import { Download, Zap, Clock, TrendingUp } from 'lucide-react';
import { formatBytes, formatDuration, formatSpeed } from '../../lib/format';
import { cn } from '../../lib/utils';
import type { DownloadProgress } from '../../api/proto/download_progress';

interface ProgressIndicatorProps {
  progress: DownloadProgress;
  compact?: boolean;
}

export function ProgressIndicator({
  progress,
  compact = false,
}: ProgressIndicatorProps) {
  const isHealthy = progress.playbackRatio >= 1.0;

  if (compact) {
    return (
      <div className="flex items-center gap-2 text-xs">
        <span
          className={cn(
            'flex items-center gap-1',
            isHealthy
              ? 'text-green-600 dark:text-green-400'
              : 'text-orange-600 dark:text-orange-400',
          )}
        >
          <Download className="h-3 w-3" />
          {formatSpeed(Number(progress.speedBytesPerSec))}
        </span>
        <span className="text-muted-foreground">
          {formatBytes(Number(progress.bytesDownloaded))}
        </span>
        <span className="flex items-center gap-1 text-muted-foreground">
          <Clock className="h-3 w-3" />
          {formatDuration(progress.durationSecs)}
        </span>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-1 p-2 bg-muted/30 rounded-md text-xs">
      <div className="flex items-center justify-between">
        <span className="flex items-center gap-1 text-muted-foreground">
          <Download className="h-3 w-3" />
          <span>{formatBytes(Number(progress.bytesDownloaded))}</span>
        </span>
        <span
          className={cn(
            'flex items-center gap-1 font-medium',
            isHealthy
              ? 'text-green-600 dark:text-green-400'
              : 'text-orange-600 dark:text-orange-400',
          )}
        >
          <Zap className="h-3 w-3" />
          {formatSpeed(Number(progress.speedBytesPerSec))}
        </span>
      </div>
      <div className="flex items-center justify-between">
        <span className="flex items-center gap-1 text-muted-foreground">
          <Clock className="h-3 w-3" />
          {formatDuration(progress.durationSecs)}
        </span>
        {progress.playbackRatio > 0 && (
          <span
            className={cn(
              'flex items-center gap-1',
              isHealthy
                ? 'text-green-600 dark:text-green-400'
                : 'text-orange-600 dark:text-orange-400',
            )}
          >
            <TrendingUp className="h-3 w-3" />
            {progress.playbackRatio.toFixed(2)}x
          </span>
        )}
      </div>
    </div>
  );
}
