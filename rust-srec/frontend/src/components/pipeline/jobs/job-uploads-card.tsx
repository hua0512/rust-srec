import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  CloudUpload,
  Copy,
  CircleCheck,
  CircleX,
  CircleMinus,
} from 'lucide-react';
import { toast } from 'sonner';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { basename, formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';
import type { UploadRecord, UploadRecordStatus } from '@/api/schemas';

const STATUS_STYLE: Record<
  UploadRecordStatus,
  { icon: typeof CircleCheck; badgeClassName: string; iconClassName: string }
> = {
  COMPLETED: {
    icon: CircleCheck,
    badgeClassName:
      'bg-emerald-500/10 text-emerald-600 border-emerald-500/20 dark:text-emerald-400',
    iconClassName: 'text-emerald-500',
  },
  FAILED: {
    icon: CircleX,
    badgeClassName: 'bg-destructive/10 text-destructive border-destructive/20',
    iconClassName: 'text-destructive',
  },
  SKIPPED: {
    icon: CircleMinus,
    badgeClassName: 'bg-muted/60 text-muted-foreground border-border/50',
    iconClassName: 'text-muted-foreground',
  },
};

function statusLabel(status: UploadRecordStatus) {
  switch (status) {
    case 'COMPLETED':
      return <Trans>Uploaded</Trans>;
    case 'FAILED':
      return <Trans>Failed</Trans>;
    case 'SKIPPED':
      return <Trans>Skipped</Trans>;
  }
}

/**
 * Per-file upload results for a job (durable `upload_records`). Rendered on
 * the job detail page only when the job produced at least one record.
 */
export function JobUploadsCard({ records }: { records: UploadRecord[] }) {
  const { i18n } = useLingui();

  const handleCopyDestination = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(i18n._(msg`Destination copied`));
    } catch {
      toast.error(i18n._(msg`Failed to copy destination`));
    }
  };

  if (records.length === 0) return null;

  return (
    <Card className="bg-card/40 backdrop-blur-sm border-border/40 shadow-sm">
      <CardHeader className="border-b border-border/40 pb-4">
        <CardTitle className="text-lg font-semibold flex items-center gap-2">
          <CloudUpload className="h-5 w-5 text-primary/70" />
          <Trans>Uploads</Trans>
          <Badge variant="secondary" className="ml-1 font-mono text-[10px]">
            {records.length}
          </Badge>
        </CardTitle>
      </CardHeader>
      <CardContent className="p-4">
        <div className="space-y-2 max-h-[280px] overflow-y-auto pr-2 custom-scrollbar">
          {records.map((record) => {
            const style = STATUS_STYLE[record.status];
            const StatusIcon = style.icon;
            return (
              <div
                key={record.id}
                className="flex items-center gap-3 p-3 rounded-xl bg-background/50 border border-border/50 hover:bg-background/80 transition-colors"
              >
                <StatusIcon
                  className={cn('h-4 w-4 shrink-0', style.iconClassName)}
                />
                <div className="flex-1 min-w-0">
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <div className="text-sm font-medium truncate">
                        {basename(record.local_path)}
                      </div>
                    </TooltipTrigger>
                    <TooltipContent className="max-w-md break-all font-mono text-xs">
                      {record.local_path}
                    </TooltipContent>
                  </Tooltip>
                  {record.remote_path && (
                    <div className="text-xs font-mono text-muted-foreground truncate">
                      {record.remote_path}
                    </div>
                  )}
                  {record.error && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <div className="text-xs text-destructive truncate">
                          {record.error}
                        </div>
                      </TooltipTrigger>
                      <TooltipContent className="max-w-md break-all text-xs">
                        {record.error}
                      </TooltipContent>
                    </Tooltip>
                  )}
                </div>
                {record.size_bytes != null && (
                  <span className="text-xs font-mono text-muted-foreground shrink-0">
                    {formatBytes(record.size_bytes)}
                  </span>
                )}
                <Badge
                  variant="outline"
                  className={cn('shrink-0 text-[10px]', style.badgeClassName)}
                >
                  {statusLabel(record.status)}
                </Badge>
                {record.remote_path && (
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 shrink-0 text-muted-foreground hover:text-foreground"
                    onClick={() =>
                      void handleCopyDestination(record.remote_path!)
                    }
                  >
                    <Copy className="h-3.5 w-3.5" />
                  </Button>
                )}
              </div>
            );
          })}
        </div>
      </CardContent>
    </Card>
  );
}
