import { useCallback, useMemo } from 'react';
import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { queryKeys } from '@/api/query-keys';
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
import { containerVariants, itemVariants } from '@/lib/animation';
import { FileVideo, Film, Layers } from 'lucide-react';
import { OutputCard } from '@/components/pipeline/outputs/output-card';
import { OutputBatchActionBar } from '@/components/pipeline/outputs/output-batch-action-bar';
import { cn } from '@/lib/utils';
import {
  useBatchActionMutation,
  useBatchSelection,
} from '@/hooks/use-batch-selection';
import { ListPagination } from '@/components/shared/list-pagination';
import { PageEmptyState } from '@/components/shared/list-panel';
import { FilterChip } from '@/components/shared/filter-pill';
import { SelectModeButton } from '@/components/shared/page-actions';
import { QueryError } from '@/components/shared/query-error';
import {
  MEDIA_FILE_TYPE_ORDER,
  getMediaFileTypeMeta,
} from '@/lib/media-file-type';
import { plural, t } from '@lingui/core/macro';
import { formatBytes } from '@/lib/format';
import { COMPACT_CARD_GRID } from '@/lib/grid';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/outputs',
)({
  component: PipelineOutputsPage,
});

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
    refetch,
  } = useQuery({
    queryKey: queryKeys.pipeline.outputs.list({
      fileType: selectedFormat,
      search: debouncedSearch,
      pageSize,
      page: currentPage,
    }),
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
    queryKey: queryKeys.pipeline.outputs.summary(debouncedSearch),
    queryFn: () =>
      getPipelineOutputSummary({
        data: { search: debouncedSearch || undefined },
      }),
    refetchInterval: 10000,
    placeholderData: keepPreviousData,
  });

  const outputs = outputsData?.items || [];
  const totalOutputs = outputsData?.total || 0;

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
    void queryClient.invalidateQueries({
      queryKey: queryKeys.pipeline.outputs.all,
    });
    void queryClient.invalidateQueries({ queryKey: queryKeys.sessions.all });
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

  const batchDeleteMutation = useBatchActionMutation({
    mutationFn: ({ ids, deleteFile }: { ids: string[]; deleteFile: boolean }) =>
      batchDeletePipelineOutputs({ data: { ids, delete_file: deleteFile } }),
    invalidate: invalidateOutputs,
    selection: { setSelectedIds, exitSelectionMode },
    successMessage: (succeeded) =>
      i18n._(
        msg`${plural(succeeded, {
          one: 'Deleted # output',
          other: 'Deleted # outputs',
        })}`,
      ),
    partialMessage: (succeeded, failed) =>
      i18n._(
        msg`${plural(succeeded, {
          one: 'Deleted # output',
          other: 'Deleted # outputs',
        })}; ${failed} failed`,
      ),
    errorMessage: i18n._(msg`Failed to delete selected outputs`),
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

  const handlePageIndexChange = useCallback(
    (index: number) => updateSearch({ page: index || undefined }),
    [updateSearch],
  );

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
              className="flex-1 md:w-64 min-w-[200px]"
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
            <SelectModeButton
              selectionMode={selectionMode}
              onToggle={toggleSelectionMode}
              label={i18n._(msg`Select outputs`)}
            />
          </>
        }
      >
        <nav className="flex items-center gap-1.5">
          <FilterChip
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
              <FilterChip
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
          {isError && !outputsData ? (
            <QueryError
              key="error"
              title={<Trans>Failed to load outputs</Trans>}
              error={error}
              onRetry={() => void refetch()}
            />
          ) : isLoading ? (
            <motion.div
              key="loading"
              initial={false}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className={cn(COMPACT_CARD_GRID, 'gap-6')}
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
                COMPACT_CARD_GRID,
                'gap-6 transition-opacity duration-200',
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
            <PageEmptyState
              key="empty"
              icon={FileVideo}
              title={
                debouncedSearch || selectedFormat ? (
                  <Trans>No outputs found</Trans>
                ) : (
                  <Trans>No media outputs yet</Trans>
                )
              }
              help={
                debouncedSearch || selectedFormat ? (
                  <Trans>Try adjusting your search or filters.</Trans>
                ) : (
                  <Trans>
                    Media outputs will appear here when pipeline jobs complete.
                  </Trans>
                )
              }
            />
          )}
        </AnimatePresence>

        <ListPagination
          pageIndex={currentPage}
          totalCount={outputsData?.total}
          pageSize={pageSize}
          onPageIndexChange={handlePageIndexChange}
          onPageSizeChange={(size) => updateSearch({ size, page: undefined })}
        />
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
