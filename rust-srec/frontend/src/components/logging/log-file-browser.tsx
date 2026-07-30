/**
 * Log file browser component with date range filtering and download capabilities.
 */
import { useState, useCallback } from 'react';
import { useQuery } from '@tanstack/react-query';
import { format } from 'date-fns';
import { motion } from 'motion/react';
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
import { toast } from 'sonner';
import {
  FileText,
  Download,
  CalendarDays,
  HardDrive,
  Archive,
  X,
  Loader2,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { formatBytes } from '@/lib/format';
import { BASE_URL } from '@/utils/env';

export function LogFileBrowser() {
  const { i18n } = useLingui();
  const [dateRange, setDateRange] = useState<DateRange | undefined>(undefined);
  const [isCalendarOpen, setIsCalendarOpen] = useState(false);
  const [isDownloadingArchive, setIsDownloadingArchive] = useState(false);

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

  // Download all logs as an archive
  const handleDownloadArchive = useCallback(async () => {
    setIsDownloadingArchive(true);

    try {
      // Get download token
      const { token } = await getLogsDownloadUrl();

      const base = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
      const url = new URL(`${base}/logging/archive`, window.location.origin);
      url.searchParams.set('token', token);
      if (fromDate) url.searchParams.set('from', fromDate);
      if (toDate) url.searchParams.set('to', toDate);

      // Stream to disk via a token-authenticated anchor navigation, matching
      // handleDownloadFile; the browser writes the zip response body directly
      // instead of buffering the whole archive in the JS heap. The download
      // attribute keeps an error response as a file download instead of
      // navigating the app to the raw error body.
      const link = document.createElement('a');
      link.href = url.toString();
      link.download = '';
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      // Anchor downloads do not expose response or completion status. The
      // browser download UI owns progress after navigation is dispatched.
      toast.info(i18n._(msg`Downloading...`));
    } catch (error: unknown) {
      console.error('Download failed:', error);
      const errorMessage =
        error instanceof Error
          ? error.message
          : i18n._(msg`Failed to download logs`);
      toast.error(errorMessage);
    } finally {
      setIsDownloadingArchive(false);
    }
  }, [fromDate, toDate, i18n]);

  // Download individual log file
  const handleDownloadFile = useCallback(
    async (file: LogFileInfo) => {
      try {
        // For individual files, we use the same archive endpoint but with specific date
        const { token } = await getLogsDownloadUrl();

        const base = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
        const url = new URL(`${base}/logging/archive`, window.location.origin);
        url.searchParams.set('token', token);
        url.searchParams.set('from', file.date);
        url.searchParams.set('to', file.date);

        const link = document.createElement('a');
        link.href = url.toString();
        link.download = '';
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
                    autoFocus
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
                disabled={isDownloadingArchive || !data?.items?.length}
                className="gap-2"
              >
                {isDownloadingArchive ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Download className="h-4 w-4" />
                )}
                <Trans>Download All</Trans>
              </Button>
            </div>
          </div>
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
                  className="opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 transition-opacity h-9 w-9"
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
