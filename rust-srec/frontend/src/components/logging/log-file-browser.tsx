/**
 * Log file browser component with date range filtering and download capabilities.
 */
import { useState, useCallback } from 'react';
import { useQuery } from '@tanstack/react-query';
import { format } from 'date-fns';
import { motion, AnimatePresence } from 'motion/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import type { DateRange } from 'react-day-picker';
import { listLogFiles, getLogsDownloadUrl } from '@/server/functions/logging';
import type { LogFileInfo } from '@/api/schemas/logging';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Calendar } from '@/components/ui/calendar';
import { Progress } from '@/components/ui/progress';
import { toast } from 'sonner';
import {
  FileText,
  Download,
  CalendarDays,
  HardDrive,
  Archive,
  X,
  Loader2,
  CheckCircle2,
  AlertCircle,
} from 'lucide-react';
import { cn } from '@/lib/utils';

/** Format bytes to human readable string */
function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

interface DownloadState {
  isDownloading: boolean;
  progress: number;
  filename?: string;
  error?: string;
  completed?: boolean;
}

export function LogFileBrowser() {
  const { i18n } = useLingui();
  const [dateRange, setDateRange] = useState<DateRange | undefined>(undefined);
  const [isCalendarOpen, setIsCalendarOpen] = useState(false);
  const [downloadState, setDownloadState] = useState<DownloadState>({
    isDownloading: false,
    progress: 0,
  });

  // Format dates for API (YYYY-MM-DD)
  const fromDate = dateRange?.from
    ? format(dateRange.from, 'yyyy-MM-dd')
    : undefined;
  const toDate = dateRange?.to ? format(dateRange.to, 'yyyy-MM-dd') : undefined;

  const { data, isLoading } = useQuery({
    queryKey: ['logging', 'files', fromDate, toDate],
    queryFn: () =>
      listLogFiles({
        data: {
          from: fromDate,
          to: toDate,
          limit: 100,
        },
      }),
  });

  const clearDateRange = useCallback(() => {
    setDateRange(undefined);
  }, []);

  // Download all logs as archive with progress tracking
  const handleDownloadArchive = useCallback(async () => {
    setDownloadState({
      isDownloading: true,
      progress: 0,
      filename: undefined,
      error: undefined,
      completed: false,
    });

    try {
      // Get download URL with date range
      const { url } = await getLogsDownloadUrl({
        data: { from: fromDate, to: toDate },
      });

      // Fetch with progress tracking
      const response = await fetch(url);

      if (!response.ok) {
        throw new Error(`Download failed: ${response.statusText}`);
      }

      const contentLength = response.headers.get('content-length');
      const total = contentLength ? parseInt(contentLength, 10) : 0;

      // Get filename from Content-Disposition header
      const disposition = response.headers.get('content-disposition');
      let filename = 'rust-srec-logs.zip';
      if (disposition) {
        const match = disposition.match(/filename="?([^";\n]+)"?/);
        if (match) filename = match[1];
      }

      setDownloadState((prev) => ({ ...prev, filename }));

      // Read the response body with progress
      const reader = response.body?.getReader();
      if (!reader) {
        throw new Error('Failed to read response body');
      }

      const chunks: Uint8Array[] = [];
      let received = 0;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        chunks.push(value);
        received += value.length;

        if (total > 0) {
          const progress = Math.round((received / total) * 100);
          setDownloadState((prev) => ({ ...prev, progress }));
        }
      }

      // Combine chunks into a single Uint8Array
      const combined = new Uint8Array(received);
      let offset = 0;
      for (const chunk of chunks) {
        combined.set(chunk, offset);
        offset += chunk.length;
      }

      // Create blob from combined array
      const blob = new Blob([combined], { type: 'application/zip' });
      const downloadUrl = window.URL.createObjectURL(blob);

      // Trigger download
      const link = document.createElement('a');
      link.href = downloadUrl;
      link.download = filename;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      window.URL.revokeObjectURL(downloadUrl);

      setDownloadState({
        isDownloading: false,
        progress: 100,
        filename,
        completed: true,
      });

      toast.success(i18n._(msg`Logs downloaded successfully`));

      // Reset state after a delay
      setTimeout(() => {
        setDownloadState({ isDownloading: false, progress: 0 });
      }, 3000);
    } catch (error: unknown) {
      console.error('Download failed:', error);
      const errorMessage =
        error instanceof Error ? error.message : 'Download failed';
      setDownloadState({
        isDownloading: false,
        progress: 0,
        error: errorMessage,
      });
      toast.error(errorMessage || i18n._(msg`Failed to download logs`));
    }
  }, [fromDate, toDate, i18n]);

  // Download individual log file
  const handleDownloadFile = useCallback(
    async (file: LogFileInfo) => {
      try {
        // For individual files, we use the same archive endpoint but with specific date
        const { url } = await getLogsDownloadUrl({
          data: { from: file.date, to: file.date },
        });

        const link = document.createElement('a');
        link.href = url;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);

        toast.success(i18n._(msg`Downloading ${file.filename}`));
      } catch (error: unknown) {
        const errorMessage =
          error instanceof Error ? error.message : 'Failed to download file';
        toast.error(errorMessage || i18n._(msg`Failed to download file`));
      }
    },
    [i18n],
  );

  // Calculate total size
  const totalSize = data?.items?.reduce((sum, f) => sum + f.size_bytes, 0) || 0;

  return (
    <Card className="border-border/40 bg-linear-to-b from-card to-card/80 shadow-lg">
      <CardHeader className="pb-4">
        <div className="flex flex-col gap-4">
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Archive className="h-5 w-5 text-primary" />
                <Trans>Log Files</Trans>
                {data && (
                  <Badge variant="outline" className="ml-2 text-xs">
                    {data.total} <Trans>files</Trans>
                  </Badge>
                )}
              </CardTitle>
              <CardDescription className="mt-1.5">
                <Trans>
                  Browse and download application log files. Filter by date
                  range or download all.
                </Trans>
              </CardDescription>
            </div>

            <div className="flex flex-wrap items-center gap-2">
              {/* Date Range Picker */}
              <Popover open={isCalendarOpen} onOpenChange={setIsCalendarOpen}>
                <PopoverTrigger asChild>
                  <Button
                    variant="outline"
                    className={cn(
                      'justify-start text-left font-normal',
                      !dateRange?.from && 'text-muted-foreground',
                    )}
                  >
                    <CalendarDays className="mr-2 h-4 w-4" />
                    {dateRange?.from ? (
                      dateRange.to ? (
                        <>
                          {format(dateRange.from, 'MMM dd, yyyy')} -{' '}
                          {format(dateRange.to, 'MMM dd, yyyy')}
                        </>
                      ) : (
                        format(dateRange.from, 'MMM dd, yyyy')
                      )
                    ) : (
                      <Trans>Select date range</Trans>
                    )}
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-auto p-0" align="end">
                  <Calendar
                    mode="range"
                    selected={dateRange}
                    onSelect={(range) => {
                      setDateRange(range);
                      if (range?.to) {
                        setIsCalendarOpen(false);
                      }
                    }}
                    disabled={(date) =>
                      date > new Date() || date < new Date('2020-01-01')
                    }
                    numberOfMonths={2}
                    initialFocus
                  />
                </PopoverContent>
              </Popover>

              {dateRange?.from && (
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={clearDateRange}
                  className="h-9 w-9"
                >
                  <X className="h-4 w-4" />
                </Button>
              )}

              {/* Download All Button */}
              <Button
                onClick={handleDownloadArchive}
                disabled={downloadState.isDownloading || !data?.items?.length}
                className="gap-2"
              >
                {downloadState.isDownloading ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Download className="h-4 w-4" />
                )}
                <Trans>Download All</Trans>
              </Button>
            </div>
          </div>

          {/* Download Progress */}
          <AnimatePresence>
            {(downloadState.isDownloading || downloadState.completed) && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="overflow-hidden"
              >
                <div className="p-4 rounded-lg bg-muted/30 border border-border/40 space-y-2">
                  <div className="flex items-center justify-between text-sm">
                    <div className="flex items-center gap-2">
                      {downloadState.completed ? (
                        <CheckCircle2 className="h-4 w-4 text-emerald-500" />
                      ) : downloadState.error ? (
                        <AlertCircle className="h-4 w-4 text-destructive" />
                      ) : (
                        <Loader2 className="h-4 w-4 animate-spin text-primary" />
                      )}
                      <span className="font-medium">
                        {downloadState.completed ? (
                          <Trans>Download complete</Trans>
                        ) : downloadState.error ? (
                          downloadState.error
                        ) : (
                          <Trans>Downloading...</Trans>
                        )}
                      </span>
                    </div>
                    {downloadState.filename && (
                      <span className="text-muted-foreground text-xs">
                        {downloadState.filename}
                      </span>
                    )}
                  </div>
                  <Progress value={downloadState.progress} className="h-2" />
                  <div className="text-xs text-muted-foreground text-right">
                    {downloadState.progress}%
                  </div>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </CardHeader>

      <CardContent>
        {/* Summary Stats */}
        {data && data.items.length > 0 && (
          <div className="flex items-center gap-4 mb-4 p-3 rounded-lg bg-muted/20 border border-border/30">
            <div className="flex items-center gap-2 text-sm">
              <HardDrive className="h-4 w-4 text-muted-foreground" />
              <span className="text-muted-foreground">
                <Trans>Total size:</Trans>
              </span>
              <span className="font-medium">{formatBytes(totalSize)}</span>
            </div>
            {dateRange?.from && (
              <div className="flex items-center gap-2 text-sm">
                <CalendarDays className="h-4 w-4 text-muted-foreground" />
                <span className="text-muted-foreground">
                  <Trans>Date range:</Trans>
                </span>
                <span className="font-medium">
                  {format(dateRange.from, 'MMM dd')}
                  {dateRange.to && ` - ${format(dateRange.to, 'MMM dd')}`}
                </span>
              </div>
            )}
          </div>
        )}

        {/* File List */}
        <div className="space-y-2 max-h-100 overflow-y-auto">
          {isLoading ? (
            // Loading skeletons
            Array.from({ length: 5 }).map((_, i) => (
              <div
                key={i}
                className="flex items-center justify-between p-3 rounded-lg border border-border/40"
              >
                <div className="flex items-center gap-3">
                  <Skeleton className="h-8 w-8 rounded" />
                  <div className="space-y-1">
                    <Skeleton className="h-4 w-48" />
                    <Skeleton className="h-3 w-24" />
                  </div>
                </div>
                <Skeleton className="h-8 w-8 rounded" />
              </div>
            ))
          ) : data?.items?.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-12 text-center">
              <FileText className="h-12 w-12 text-muted-foreground/30 mb-4" />
              <p className="text-muted-foreground">
                <Trans>No log files found</Trans>
              </p>
              {dateRange?.from && (
                <p className="text-sm text-muted-foreground/60 mt-1">
                  <Trans>Try adjusting the date range</Trans>
                </p>
              )}
            </div>
          ) : (
            data?.items?.map((file, index) => (
              <motion.div
                key={file.filename}
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: index * 0.03 }}
                className="group flex items-center justify-between p-3 rounded-lg border border-border/40 bg-muted/20 hover:bg-muted/40 transition-colors"
              >
                <div className="flex items-center gap-3 min-w-0">
                  <div className="flex items-center justify-center h-10 w-10 rounded-lg bg-primary/10 text-primary shrink-0">
                    <FileText className="h-5 w-5" />
                  </div>
                  <div className="min-w-0">
                    <p className="font-mono text-sm font-medium truncate">
                      {file.filename}
                    </p>
                    <div className="flex items-center gap-3 text-xs text-muted-foreground">
                      <span>{file.date}</span>
                      <span className="flex items-center gap-1">
                        <HardDrive className="h-3 w-3" />
                        {formatBytes(file.size_bytes)}
                      </span>
                    </div>
                  </div>
                </div>

                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => handleDownloadFile(file)}
                  className="opacity-0 group-hover:opacity-100 transition-opacity h-9 w-9"
                  title={i18n._(msg`Download ${file.filename}`)}
                >
                  <Download className="h-4 w-4" />
                </Button>
              </motion.div>
            ))
          )}
        </div>
      </CardContent>
    </Card>
  );
}
