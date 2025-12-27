import { useMemo, useState, useEffect } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useQuery, keepPreviousData } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listPipelineOutputs } from '@/server/functions';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans, useLingui } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination';
import { FileVideo, Search, AlertCircle, Film } from 'lucide-react';
import { OutputCard } from '@/components/pipeline/outputs/output-card';

export const Route = createFileRoute('/_authed/_dashboard/pipeline/outputs')({
  component: PipelineOutputsPage,
});

const PAGE_SIZES = [12, 24, 48, 96];

import { formatBytes } from '@/lib/format';

function PipelineOutputsPage() {
  const { i18n } = useLingui();
  const [selectedFormat, setSelectedFormat] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const [pageSize, setPageSize] = useState(24);
  const [currentPage, setCurrentPage] = useState(0);

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(searchQuery);
      setCurrentPage(0);
    }, 300);
    return () => clearTimeout(timer);
  }, [searchQuery]);

  // Reset page when format changes
  const handleFormatChange = (format: string | null) => {
    setSelectedFormat(format);
    setCurrentPage(0);
  };

  const {
    data: outputsData,
    isLoading,
    isError,
    error,
  } = useQuery({
    queryKey: [
      'pipeline',
      'outputs',
      selectedFormat,
      debouncedSearch,
      pageSize,
      currentPage,
    ],
    queryFn: () =>
      listPipelineOutputs({
        data: {
          search: debouncedSearch || undefined,
          limit: pageSize,
          offset: currentPage * pageSize,
        },
      }),
    refetchInterval: 10000,
    placeholderData: keepPreviousData,
  });

  const outputs = outputsData?.items || [];
  const totalOutputs = outputsData?.total || 0;
  const totalSize = outputs.reduce(
    (acc, output) => acc + output.file_size_bytes,
    0,
  ); // Note: Server doesn't return total size of all filtered items yet, this is just page total
  const totalPages = Math.ceil(totalOutputs / pageSize);

  // Client-side format filtering since API doesn't support it yet
  const displayedOutputs = useMemo(() => {
    let result = outputs;
    if (selectedFormat) {
      result = result.filter(
        (output) =>
          output.format.toLowerCase() === selectedFormat.toLowerCase(),
      );
    }
    return result;
  }, [outputs, selectedFormat]);

  // Get unique formats for the filter (from current page only - limitation)
  const availableFormats = useMemo(() => {
    const formats = new Set(outputs.map((o) => o.format.toLowerCase()));
    return Array.from(formats).sort();
  }, [outputs]);

  // Memoize pagination pages calculation
  const paginationPages = useMemo(() => {
    const pages: (number | 'ellipsis')[] = [];
    if (totalPages <= 7) {
      for (let i = 0; i < totalPages; i++) pages.push(i);
    } else {
      pages.push(0);
      if (currentPage > 2) pages.push('ellipsis');
      for (
        let i = Math.max(1, currentPage - 1);
        i <= Math.min(totalPages - 2, currentPage + 1);
        i++
      ) {
        pages.push(i);
      }
      if (currentPage < totalPages - 3) pages.push('ellipsis');
      pages.push(totalPages - 1);
    }
    return pages;
  }, [totalPages, currentPage]);

  if (isError) {
    return (
      <div className="space-y-8 p-6 md:p-10 max-w-7xl mx-auto">
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertTitle>
            <Trans>Error</Trans>
          </AlertTitle>
          <AlertDescription>
            <Trans>Failed to load outputs: {error.message}</Trans>
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <div className="min-h-screen space-y-6">
      {/* Header */}
      <DashboardHeader
        icon={Film}
        title={<Trans>Media Outputs</Trans>}
        subtitle={
          <Trans>Browse generated media artifacts from pipeline jobs</Trans>
        }
        actions={
          <>
            {/* Search Input */}
            <div className="relative flex-1 md:w-64">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
              <Input
                placeholder={t`Search outputs...`}
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="pl-9 h-9"
              />
            </div>
            <Badge
              variant="secondary"
              className="h-9 px-3 text-sm whitespace-nowrap"
            >
              {i18n.number(totalOutputs)} <Trans>files</Trans>
            </Badge>
            <Badge
              variant="outline"
              className="h-9 px-3 text-sm whitespace-nowrap"
            >
              {formatBytes(totalSize)}
            </Badge>
          </>
        }
      >
        <nav className="flex items-center gap-1">
          <button
            onClick={() => handleFormatChange(null)}
            className={`relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 ${
              selectedFormat === null
                ? 'bg-primary text-primary-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground hover:bg-muted'
            }`}
          >
            <span className="relative z-10 flex items-center gap-1.5">
              <Trans>All</Trans>
            </span>
          </button>

          {availableFormats.map((format) => (
            <button
              key={format}
              onClick={() => handleFormatChange(format)}
              className={`relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 ${
                selectedFormat === format
                  ? 'bg-primary text-primary-foreground shadow-sm'
                  : 'text-muted-foreground hover:text-foreground hover:bg-muted'
              }`}
            >
              <span className="relative z-10 uppercase">{format}</span>
            </button>
          ))}
        </nav>
      </DashboardHeader>

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isLoading ? (
            <motion.div
              key="loading"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {[1, 2, 3, 4, 5, 6, 7, 8].map((i) => (
                <div
                  key={i}
                  className="h-[220px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm"
                >
                  <div className="flex justify-between items-start">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <Skeleton className="h-6 w-12" />
                  </div>
                  <div className="space-y-2 pt-2">
                    <Skeleton className="h-6 w-3/4" />
                    <Skeleton className="h-12 w-full rounded-md" />
                  </div>
                  <div className="pt-4 mt-auto grid grid-cols-2 gap-2">
                    <Skeleton className="h-12 w-full rounded-md" />
                    <Skeleton className="h-12 w-full rounded-md" />
                  </div>
                </div>
              ))}
            </motion.div>
          ) : displayedOutputs.length > 0 ? (
            <motion.div
              key="list"
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.3 }}
            >
              {displayedOutputs.map((output, index) => (
                <motion.div
                  key={output.id}
                  initial={{ opacity: 0, y: 20 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{
                    duration: 0.3,
                    delay: Math.min(index * 0.05, 0.3),
                  }}
                >
                  <OutputCard output={output} />
                </motion.div>
              ))}
            </motion.div>
          ) : (
            <motion.div
              key="empty"
              initial={{ opacity: 0, scale: 0.95 }}
              animate={{ opacity: 1, scale: 1 }}
              className="flex flex-col items-center justify-center py-32 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5 backdrop-blur-sm shadow-sm"
            >
              <div className="p-6 bg-primary/5 rounded-full ring-1 ring-primary/10">
                <FileVideo className="h-16 w-16 text-primary/60" />
              </div>
              <div className="space-y-2 max-w-md">
                <h3 className="font-semibold text-2xl tracking-tight">
                  {debouncedSearch || selectedFormat ? (
                    <Trans>No outputs found</Trans>
                  ) : (
                    <Trans>No media outputs yet</Trans>
                  )}
                </h3>
                <p className="text-muted-foreground">
                  {debouncedSearch || selectedFormat ? (
                    <Trans>Try adjusting your search or filters.</Trans>
                  ) : (
                    <Trans>
                      Media outputs will appear here when pipeline jobs
                      complete.
                    </Trans>
                  )}
                </p>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Pagination Controls */}
        {totalPages > 1 && (
          <div className="flex items-center justify-between mt-8 pt-6 border-t">
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">
                <Trans>Per page:</Trans>
              </span>
              <Select
                value={pageSize.toString()}
                onValueChange={(value) => {
                  setPageSize(Number(value));
                  setCurrentPage(0);
                }}
              >
                <SelectTrigger className="w-20 h-8">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PAGE_SIZES.map((size) => (
                    <SelectItem key={size} value={size.toString()}>
                      {size}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <Pagination>
              <PaginationContent>
                <PaginationItem>
                  <PaginationPrevious
                    onClick={() => setCurrentPage((p) => Math.max(0, p - 1))}
                    className={
                      currentPage === 0
                        ? 'pointer-events-none opacity-50'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>

                {paginationPages.map((page, idx) =>
                  page === 'ellipsis' ? (
                    <PaginationItem key={`ellipsis-${idx}`}>
                      <PaginationEllipsis />
                    </PaginationItem>
                  ) : (
                    <PaginationItem key={page}>
                      <PaginationLink
                        isActive={currentPage === page}
                        onClick={() => setCurrentPage(page)}
                        className="cursor-pointer"
                      >
                        {page + 1}
                      </PaginationLink>
                    </PaginationItem>
                  ),
                )}

                <PaginationItem>
                  <PaginationNext
                    onClick={() =>
                      setCurrentPage((p) => Math.min(totalPages - 1, p + 1))
                    }
                    className={
                      currentPage >= totalPages - 1
                        ? 'pointer-events-none opacity-50'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>
              </PaginationContent>
            </Pagination>
          </div>
        )}
      </div>
    </div>
  );
}
