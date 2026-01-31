import { memo } from 'react';
import { cn } from '@/lib/utils';

export const PayloadPreview = memo(({ payload }: { payload: string }) => {
  try {
    const parsed = JSON.parse(payload);
    const inner =
      parsed.StreamOnline ||
      parsed.StreamOffline ||
      parsed.DownloadStarted ||
      parsed.DownloadCompleted ||
      parsed.DownloadError ||
      parsed.SegmentStarted ||
      parsed.SegmentCompleted ||
      parsed.DownloadCancelled ||
      parsed.DownloadRejected ||
      parsed.ConfigUpdated ||
      parsed.PipelineStarted ||
      parsed.PipelineCompleted ||
      parsed.PipelineFailed ||
      parsed.PipelineCancelled ||
      parsed.FatalError ||
      parsed.OutOfSpace ||
      parsed.PipelineQueueWarning ||
      parsed.PipelineQueueCritical ||
      parsed.SystemStartup ||
      parsed.SystemShutdown ||
      (parsed.Credential && parsed.Credential.event) ||
      {};

    const variant = Object.keys(parsed)[0];
    const fields: {
      label: string;
      value: string | number;
      color?: string;
      fullWidth?: boolean;
    }[] = [];

    // Extract detailed fields
    if (inner.streamer_name)
      fields.push({ label: 'Streamer', value: inner.streamer_name });
    if (inner.job_type) fields.push({ label: 'Job', value: inner.job_type });
    if (inner.error_type || inner.error) {
      fields.push({
        label: 'Error',
        value:
          inner.error_type || inner.error || inner.reason || 'Unknown error',
        color: 'text-destructive font-medium',
        fullWidth: true,
      });
    }

    if (inner.title)
      fields.push({ label: 'Title', value: inner.title, fullWidth: true });
    if (inner.category)
      fields.push({ label: 'Category', value: inner.category });
    if (inner.platform)
      fields.push({ label: 'Platform', value: inner.platform });

    if (inner.output_path || inner.path) {
      fields.push({
        label: 'Path',
        value: inner.output_path || inner.path,
        color: 'text-emerald-600 dark:text-emerald-400 font-mono text-[10px]',
        fullWidth: true,
      });
    }

    if (inner.duration_secs !== undefined) {
      const mins = Math.floor(inner.duration_secs / 60);
      const secs = inner.duration_secs % 60;
      fields.push({
        label: 'Duration',
        value: mins > 0 ? `${mins}m ${secs}s` : `${secs}s`,
      });
    }

    if (inner.file_size_bytes !== undefined) {
      const mb = (inner.file_size_bytes / (1024 * 1024)).toFixed(1);
      fields.push({ label: 'Size', value: `${mb} MB` });
    }

    if (inner.version) fields.push({ label: 'Version', value: inner.version });

    // If no fields extracted, show variant name
    if (fields.length === 0 && variant) {
      return (
        <span className="text-[11px] font-semibold text-muted-foreground/60 uppercase tracking-wider bg-muted/30 px-2 py-0.5 rounded-md">
          {variant.replace(/([A-Z])/g, ' $1').trim()}
        </span>
      );
    }

    const displayFields = fields.slice(0, 6);

    return (
      <div className="grid grid-cols-2 gap-x-4 gap-y-1.5 mt-1">
        {displayFields.map((field, idx) => (
          <div
            key={idx}
            className={cn(
              'flex items-baseline gap-1.5 text-[11px] min-w-0',
              field.fullWidth ? 'col-span-2' : 'col-span-1',
            )}
          >
            <span className="text-muted-foreground/50 font-medium shrink-0 tabular-nums uppercase text-[9px] tracking-tight">
              {field.label}:
            </span>
            <span
              className={cn(
                'truncate leading-tight',
                field.color || 'text-foreground/70',
              )}
            >
              {field.value}
            </span>
          </div>
        ))}
      </div>
    );
  } catch {
    return (
      <p className="text-xs text-muted-foreground line-clamp-2 leading-relaxed">
        {payload.slice(0, 100)}
      </p>
    );
  }
});
PayloadPreview.displayName = 'PayloadPreview';
