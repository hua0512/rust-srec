import { Download as DownloadIcon, Zap, Clock, TrendingUp } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { formatBytes, formatDuration, formatSpeed } from '../../lib/format';
import { cn } from '../../lib/utils';
import type { Download } from '@/store/downloads';
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip';
import { StatusInfoTooltip } from '../shared/status-info-tooltip';

interface ProgressIndicatorProps {
  progress: Download;
  compact?: boolean;
}

export function ProgressIndicator({
  progress,
  compact = false,
}: ProgressIndicatorProps) {
  const { i18n } = useLingui();
  const isHealthy = progress.playbackRatio >= 1.0;
  const cdnHost = progress.cdnHost || '';
  const tooltipTheme = isHealthy ? 'blue' : 'orange';

  const trigger = compact ? (
    <div className="flex items-center gap-2 text-xs">
      <div
        className={cn(
          'flex items-center gap-1',
          isHealthy
            ? 'text-green-600 dark:text-green-400'
            : 'text-orange-600 dark:text-orange-400',
        )}
      >
        <DownloadIcon className="h-3 w-3" />
        {formatSpeed(Number(progress.speedBytesPerSec))}
      </div>
      <div className="text-muted-foreground">
        {formatBytes(Number(progress.bytesDownloaded))}
      </div>
      <div className="flex items-center gap-1 text-muted-foreground">
        <Clock className="h-3 w-3" />
        {formatDuration(progress.durationSecs)}
      </div>
    </div>
  ) : (
    <div className="flex flex-col gap-1 p-2 bg-muted/30 rounded-md text-xs">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1 text-muted-foreground">
          <DownloadIcon className="h-3 w-3" />
          <div>{formatBytes(Number(progress.bytesDownloaded))}</div>
        </div>
        <div
          className={cn(
            'flex items-center gap-1 font-medium',
            isHealthy
              ? 'text-green-600 dark:text-green-400'
              : 'text-orange-600 dark:text-orange-400',
          )}
        >
          <Zap className="h-3 w-3" />
          {formatSpeed(Number(progress.speedBytesPerSec))}
        </div>
      </div>
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1 text-muted-foreground">
          <Clock className="h-3 w-3" />
          {formatDuration(progress.durationSecs)}
        </div>
        {progress.playbackRatio > 0 && (
          <div
            className={cn(
              'flex items-center gap-1',
              isHealthy
                ? 'text-green-600 dark:text-green-400'
                : 'text-orange-600 dark:text-orange-400',
            )}
          >
            <TrendingUp className="h-3 w-3" />
            {progress.playbackRatio.toFixed(2)}x
          </div>
        )}
      </div>
    </div>
  );

  return (
    <Tooltip delayDuration={100}>
      <TooltipTrigger asChild>
        <button
          type="button"
          className={cn(
            // Reset native button styling to avoid unexpected padding/line-height differences.
            'appearance-none p-0 border-0 bg-transparent text-left rounded-md',
            'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background',
            compact ? 'inline-flex' : 'block w-full',
          )}
          aria-label={i18n._(msg`Show download details`)}
        >
          {trigger}
        </button>
      </TooltipTrigger>
      <TooltipContent
        side="top"
        className="p-0 bg-background text-foreground border border-border/50 shadow-xl backdrop-blur-md overflow-hidden rounded-xl"
      >
        <StatusInfoTooltip
          icon={<DownloadIcon className="w-4 h-4" />}
          title={
            <Trans render={({ translation }) => <>{translation}</>}>
              Download
            </Trans>
          }
          subtitle={
            <div className="font-mono">
              {formatSpeed(Number(progress.speedBytesPerSec))} ·{' '}
              {formatBytes(Number(progress.bytesDownloaded))}
            </div>
          }
          theme={tooltipTheme}
        >
          <div className="grid grid-cols-2 gap-2 text-xs">
            <div className="text-muted-foreground">
              <Trans render={({ translation }) => <>{translation}</>}>
                Duration
              </Trans>
            </div>
            <div className="font-mono text-foreground/90">
              {formatDuration(progress.durationSecs)}
            </div>
            <div className="text-muted-foreground">
              <Trans render={({ translation }) => <>{translation}</>}>
                Ratio
              </Trans>
            </div>
            <div className="font-mono text-foreground/90">
              {progress.playbackRatio > 0
                ? progress.playbackRatio.toFixed(2) + 'x'
                : '-'}
            </div>
            <div className="text-muted-foreground">
              <Trans render={({ translation }) => <>{translation}</>}>
                CDN
              </Trans>
            </div>
            <div className="font-mono text-foreground/90 break-all">
              {cdnHost || '-'}
            </div>
            {progress.downloadUrl && (
              <>
                <div className="text-muted-foreground">
                  <Trans render={({ translation }) => <>{translation}</>}>
                    URL
                  </Trans>
                </div>
                <div
                  className="font-mono text-foreground/90 break-all max-w-[200px] truncate"
                  title={progress.downloadUrl}
                >
                  {progress.downloadUrl}
                </div>
              </>
            )}
          </div>
        </StatusInfoTooltip>
      </TooltipContent>
    </Tooltip>
  );
}
