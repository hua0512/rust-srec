import { useCallback, useMemo, type ReactNode } from 'react';
import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import {
  listPipelineOutputs,
  getPipelineOutputSummary,
  deletePipelineOutput,
  batchDeletePipelineOutputs,
} from '@/server/functions';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { CardSkeleton } from '@/components/shared/card-skeleton';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Badge } from '@/components/ui/badge';
import { SearchInput } from '@/components/shared/search-input';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { containerVariants, itemVariants } from '@/lib/animation';
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
import {
  FileVideo,
  AlertCircle,
  Film,
  ListChecks,
  Layers,
  type LucideIcon,
} from 'lucide-react';
import { OutputCard } from '@/components/pipeline/outputs/output-card';
import { OutputBatchActionBar } from '@/components/pipeline/outputs/output-batch-action-bar';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useBatchSelection } from '@/hooks/use-batch-selection';
import {
  MEDIA_FILE_TYPE_ORDER,
  getMediaFileTypeMeta,
} from '@/lib/media-file-type';
import { plural, t } from '@lingui/core/macro';
import { formatBytes } from '@/lib/format';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/outputs',
)({
  component: PipelineOutputsPage,
});

const PAGE_SIZES = [12, 24, 48, 96];

/**
 * One pill in the file-type filter row, matching the pipeline jobs status
 * filter. `count` arrives pre-formatted and is omitted while the outputs
 * summary is still loading, so the pill never briefly claims zero.
 */
function TypeFilterPill({
  icon: Icon,
  label,
  count,
  isActive,
  onClick,
}: {
  icon: LucideIcon;
  label: ReactNode;
  count?: string;
  isActive: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      aria-pressed={isActive}
      className={cn(
        'relative flex shrink-0 items-center gap-1.5 rounded-full px-3.5 py-1.5 text-xs sm:text-sm font-medium shadow-sm ring-1 transition-all duration-200',
        isActive
          ? 'bg-primary text-primary-foreground ring-primary'
          : 'bg-background text-muted-foreground ring-border/50 hover:bg-muted hover:text-foreground',
      )}
    >
      <Icon className="h-4 w-4" />
      <span className="whitespace-nowrap">{label}</span>
      {count !== undefined && (
        <span className="tabular-nums opacity-70">{count}</span>
      )}
    </button>
  );
}

function PipelineOutputsPage() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const search = Route.useSearch();
  const updateSearch = useUpdateSearch<typeof search>();

  // Search, file-type filter, and pagination live in the URL so they persist
  // across navigation away from this page and reloads.
  const selectedFormat = search.format ?? null;
  const debouncedSearch = search.q ?? '';
  const pageSize = search.size ?? 24;
  const currentPage = search.page ?? 0;

  // Reset page when the type changes: the previous offset almost never lands on
  // a valid page of the narrower result set.
  const handleFormatChange = (format: string | null) => {
    updateSearch({ format: format ?? undefined, page: undefined });
  };

  const {
    data: outputsData,
    isLoading,
    isPlaceholderData,
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
          file_type: selectedFormat || undefined,
          search: debouncedSearch || undefined,
          limit: pageSize,
          offset: currentPage * pageSize,
        },
      }),
    refetchInterval: 10000,
    placeholderData: keepPreviousData,
  });

  // Deliberately not keyed on the selected type: the endpoint reports every
  // type, so switching tabs reuses this result instead of refetching.
  const { data: summary } = useQuery({
    queryKey: ['pipeline', 'outputs', 'summary', debouncedSearch],
    queryFn: () =>
      getPipelineOutputSummary({
        data: { search: debouncedSearch || undefined },
      }),
    refetchInterval: 10000,
    placeholderData: keepPreviousData,
  });

  const outputs = outputsData?.items || [];
  const totalOutputs = outputsData?.total || 0;
  const totalPages = Math.ceil(totalOutputs / pageSize);

  // `Intl.NumberFormat` rather than `i18n.number`, which Lingui v6 deprecates.
  const numberFormat = useMemo(
    () => new Intl.NumberFormat(i18n.locale),
    [i18n.locale],
  );

  // Counts and sizes come from the summary so they describe every match, not
  // just the rows on this page.
  const countsByType = useMemo(
    () =>
      new Map(summary?.by_type.map((entry) => [entry.file_type, entry]) ?? []),
    [summary],
  );
  const filteredSize = selectedFormat
    ? countsByType.get(selectedFormat)?.size_bytes
    : summary?.total_size_bytes;

  // Ordered by `MEDIA_FILE_TYPE_ORDER`, then any type the backend reports that
  // the type map does not know, so its outputs stay reachable.
  const typeFilters = useMemo(() => {
    const present = new Set(summary?.by_type.map((entry) => entry.file_type));
    const known = MEDIA_FILE_TYPE_ORDER.filter((type) => present.has(type));
    const unknown = [...present]
      .filter((type) => !MEDIA_FILE_TYPE_ORDER.includes(type))
      .sort();
    return [...known, ...unknown];
  }, [summary]);

  const pageIds = useMemo(() => outputs.map((output) => output.id), [outputs]);

  const {
    selectionMode,
    selectedIds,
    setSelectedIds,
    allPageSelected,
    handleSelectionChange,
    selectPage,
    clearSelection,
    toggleSelectionMode,
    exitSelectionMode,
  } = useBatchSelection({
    pageIds,
    scope: [currentPage, pageSize, debouncedSearch, selectedFormat ?? ''].join(
      '|',
    ),
  });

  // Deleting an output decrements its session's total size, so the sessions
  // list has to be refetched alongside this page.
  const invalidateOutputs = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ['pipeline', 'outputs'] });
    void queryClient.invalidateQueries({ queryKey: ['sessions'] });
  }, [queryClient]);

  const deleteMutation = useMutation({
    mutationFn: ({ id, deleteFile }: { id: string; deleteFile: boolean }) =>
      deletePipelineOutput({ data: { id, deleteFile } }),
    onSuccess: (result) => {
      invalidateOutputs();
      toast.success(
        result.file_deleted
          ? i18n._(msg`Output and its file deleted`)
          : i18n._(msg`Output deleted`),
      );
    },
    onError: (error) =>
      toast.error(
        error instanceof Error
          ? error.message
          : i18n._(msg`Failed to delete output`),
      ),
  });

  const batchDeleteMutation = useMutation({
    mutationFn: ({ ids, deleteFile }: { ids: string[]; deleteFile: boolean }) =>
      batchDeletePipelineOutputs({ data: { ids, delete_file: deleteFile } }),
    onSuccess: (result) => {
      invalidateOutputs();

      if (result.failed === 0) {
        toast.success(
          i18n._(msg`Successfully deleted ${result.succeeded} outputs`),
        );
        exitSelectionMode();
        return;
      }

      // Keep only the failures selected so a retry targets exactly the outputs
      // that were not removed.
      const failedResults = result.results.filter((item) => !item.success);
      setSelectedIds(new Set(failedResults.map((item) => item.id)));
      toast.warning(
        i18n._(
          msg`Deleted ${result.succeeded} outputs; ${result.failed} failed`,
        ),
        {
          description: failedResults
            .slice(0, 3)
            .map((item) => item.error)
            .filter(Boolean)
            .join('; '),
        },
      );
    },
    onError: (error) =>
      toast.error(
        error instanceof Error
          ? error.message
          : i18n._(msg`Failed to delete selected outputs`),
      ),
  });

  const deleteMutate = deleteMutation.mutate;
  const handleDeleteOutput = useCallback(
    (id: string, deleteFile: boolean) => deleteMutate({ id, deleteFile }),
    [deleteMutate],
  );

  const handleBatchDelete = useCallback(
    (deleteFile: boolean) => {
      if (selectedIds.size === 0 || batchDeleteMutation.isPending) return;
      batchDeleteMutation.mutate({
        ids: Array.from(selectedIds),
        deleteFile,
      });
    },
    [selectedIds, batchDeleteMutation],
  );

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
            <SearchInput
              defaultValue={debouncedSearch}
              onSearch={(value) =>
                updateSearch({ q: value || undefined, page: undefined })
              }
              placeholder={i18n._(msg`Search outputs...`)}
              className="flex-1 md:w-64"
            />
            <Badge
              variant="secondary"
              className="h-9 px-3 text-sm whitespace-nowrap tabular-nums"
            >
              {t(i18n)`${plural(totalOutputs, {
                one: '# file',
                other: '# files',
              })}`}
            </Badge>
            {/* Hidden rather than "0 B" until the summary lands: a zero here
                would read as an empty library. */}
            {filteredSize !== undefined && (
              <Badge
                variant="outline"
                className="h-9 px-3 text-sm whitespace-nowrap tabular-nums"
              >
                {formatBytes(filteredSize)}
              </Badge>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={toggleSelectionMode}
              aria-pressed={selectionMode}
              aria-label={i18n._(msg`Select outputs`)}
              className={cn(
                'h-9 gap-2 whitespace-nowrap rounded-full px-3',
                selectionMode &&
                  'border-primary/50 bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary',
              )}
            >
              <ListChecks className="h-4 w-4" />
              <Trans>Select</Trans>
            </Button>
          </>
        }
      >
        <nav className="flex items-center gap-1.5">
          <TypeFilterPill
            icon={Layers}
            label={<Trans>All</Trans>}
            count={
              summary ? numberFormat.format(summary.total_count) : undefined
            }
            isActive={selectedFormat === null}
            onClick={() => handleFormatChange(null)}
          />

          {typeFilters.map((fileType) => {
            const meta = getMediaFileTypeMeta(fileType);
            const count = countsByType.get(fileType)?.count;
            return (
              <TypeFilterPill
                key={fileType}
                icon={meta.icon}
                label={i18n._(meta.label)}
                count={
                  count === undefined ? undefined : numberFormat.format(count)
                }
                isActive={selectedFormat === fileType}
                onClick={() => handleFormatChange(fileType)}
              />
            );
          })}
        </nav>
      </DashboardHeader>

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isLoading ? (
            <motion.div
              key="loading"
              initial={false}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {[1, 2, 3, 4, 5, 6, 7, 8].map((i) => (
                <CardSkeleton key={i}>
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
                </CardSkeleton>
              ))}
            </motion.div>
          ) : outputs.length > 0 ? (
            <motion.div
              key="list"
              // `keepPreviousData` keeps the old page on screen while the next
              // one loads; dimming it marks the rows as not yet the ones asked
              // for, and blocks clicks on cards that are about to be replaced.
              className={cn(
                'grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6 transition-opacity duration-200',
                isPlaceholderData && 'pointer-events-none opacity-60',
              )}
              aria-busy={isPlaceholderData}
              variants={containerVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              {outputs.map((output) => (
                <motion.div key={output.id} variants={itemVariants}>
                  <OutputCard
                    output={output}
                    onDelete={handleDeleteOutput}
                    selectionMode={selectionMode}
                    isSelected={selectedIds.has(output.id)}
                    onSelectChange={handleSelectionChange}
                  />
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
                  updateSearch({ size: Number(value), page: undefined });
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
                    onClick={() =>
                      updateSearch({ page: Math.max(0, currentPage - 1) })
                    }
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
                        onClick={() => updateSearch({ page })}
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
                      updateSearch({
                        page: Math.min(totalPages - 1, currentPage + 1),
                      })
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

      <AnimatePresence>
        {selectionMode && (
          <OutputBatchActionBar
            selectedCount={selectedIds.size}
            pageCount={outputs.length}
            allPageSelected={allPageSelected}
            isPending={batchDeleteMutation.isPending}
            onSelectPage={selectPage}
            onClearSelection={clearSelection}
            onDelete={handleBatchDelete}
            onExit={toggleSelectionMode}
          />
        )}
      </AnimatePresence>
    </div>
  );
}
