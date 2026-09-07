import { memo } from 'react';
import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { cn } from '@/lib/utils';
import { formatBytes, formatDuration } from '@/lib/format';

export const PayloadPreview = memo(({ payload }: { payload: string }) => {
  const { i18n } = useLingui();

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
      label: MessageDescriptor;
      value: string | number;
      color?: string;
      fullWidth?: boolean;
    }[] = [];

    // Extract detailed fields
    if (inner.streamer_name)
      fields.push({ label: msg`Streamer`, value: inner.streamer_name });
    if (inner.job_type) fields.push({ label: msg`Job`, value: inner.job_type });
    if (inner.error_type || inner.error) {
      fields.push({
        label: msg`Error`,
        value:
          inner.error_type ||
          inner.error ||
          inner.reason ||
          i18n._(msg`Unknown error`),
        color: 'text-destructive font-medium',
        fullWidth: true,
      });
    }

    if (inner.title)
      fields.push({ label: msg`Title`, value: inner.title, fullWidth: true });
    if (inner.category)
      fields.push({ label: msg`Category`, value: inner.category });
    if (inner.platform)
      fields.push({ label: msg`Platform`, value: inner.platform });

    if (inner.output_path || inner.path) {
      fields.push({
        label: msg`Path`,
        value: inner.output_path || inner.path,
        color: 'text-emerald-600 dark:text-emerald-400 font-mono text-[10px]',
        fullWidth: true,
      });
    }

    if (inner.duration_secs !== undefined) {
      fields.push({
        label: msg`Duration`,
        value: formatDuration(inner.duration_secs, {
          decimals: 0,
          nullValue: '0s',
        }),
      });
    }

    if (inner.file_size_bytes !== undefined) {
      fields.push({
        label: msg`Size`,
        value: formatBytes(inner.file_size_bytes, { decimals: 1 }),
      });
    }

    if (inner.version)
      fields.push({ label: msg`Version`, value: inner.version });

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
              {i18n._(field.label)}:
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
