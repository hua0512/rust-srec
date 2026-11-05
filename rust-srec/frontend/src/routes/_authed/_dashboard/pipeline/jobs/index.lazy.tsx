import { useMemo, useCallback, memo } from 'react';
import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { queryKeys } from '@/api/query-keys';
import { pipelineStatsQueryOptions } from '@/api/dashboard';
import {
  listPipelines,
  cancelDag,
  deletePipeline,
  retryAllFailedPipelines,
  batchPipelines,
} from '@/server/functions';
import type { BatchDagAction, DagStatus, DagSummary } from '@/api/schemas';
import { Card } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg, plural } from '@lingui/core/macro';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { isDagAlreadyTerminalError } from '@/lib/api-error';
import { FAB_ANCHOR } from '@/components/shared/save-fab';
import { containerVariants, itemVariants } from '@/lib/animation';
import {
  Activity,
  RefreshCw,
  XCircle,
  CheckCircle2,
  Clock,
  AlertCircle,
  Timer,
  ListTodo,
  Plus,
} from 'lucide-react';
import { toast } from 'sonner';
import { PipelineSummaryCard } from '@/components/pipeline/jobs/pipeline-summary-card';
import { PipelineBatchActionBar } from '@/components/pipeline/jobs/pipeline-batch-action-bar';
import { SearchInput } from '@/components/shared/search-input';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { useUpdateSearch } from '@/hooks/use-update-search';
import {
  useBatchActionMutation,
  useBatchSelection,
} from '@/hooks/use-batch-selection';
import { ListPagination } from '@/components/shared/list-pagination';
import { PageEmptyState } from '@/components/shared/list-panel';
import { FilterChip } from '@/components/shared/filter-pill';
import { SelectModeButton } from '@/components/shared/page-actions';
import { QueryError } from '@/components/shared/query-error';
import { formatDuration } from '@/lib/format';
import { CARD_GRID } from '@/lib/grid';

export const Route = createLazyFileRoute('/_authed/_dashboard/pipeline/jobs/')({
  component: PipelineJobsPage,
});

function PipelineJobsPage() {
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  // Read state from URL search params
  const search = Route.useSearch();
  const { q, status, page, size } = search;

  // Derive state from URL params with defaults
  const selectedStatus = status ?? null;
  const currentPage = page ?? 0;
  const pageSize = size ?? 24;

  // Memoize STATUS_FILTERS to prevent recreation on every render
  const STATUS_FILTERS = useMemo(
    () =>
      [
        { value: null, label: i18n._(msg`All`), icon: ListTodo },
        { value: 'PENDING', label: i18n._(msg`Pending`), icon: Clock },
        {
          value: 'PROCESSING',
          label: i18n._(msg`Processing`),
          icon: Activity,
        },
        {
          value: 'COMPLETED',
          label: i18n._(msg`Completed`),
          icon: CheckCircle2,
        },
        { value: 'FAILED', label: i18n._(msg`Failed`), icon: XCircle },
        {
          value: 'CANCELLED',
          label: i18n._(msg`Cancelled`),
          icon: AlertCircle,
        },
      ] as const,
    [i18n],
  );

  const updateSearch = useUpdateSearch<typeof search>();

  // Reset page when status changes
  const handleStatusChange = useCallback(
    (newStatus: DagStatus | null) => {
      updateSearch({ status: newStatus ?? undefined, page: undefined });
    },
    [updateSearch],
  );

  const { data: stats, isLoading: isStatsLoading } = useQuery({
    ...pipelineStatsQueryOptions,
    refetchInterval: 10000, // Reduced from 5s to 10s
    staleTime: 5000, // Data stays fresh for 5s
    refetchIntervalInBackground: false, // Pause polling when tab is hidden
  });

  const {
    data: pipelinesData,
    isLoading: isPipelinesLoading,
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: queryKeys.pipeline.dags.list({
      status: selectedStatus,
      search: q,
      pageSize,
      page: currentPage,
    }),
    queryFn: () =>
      listPipelines({
        data: {
          status: selectedStatus || undefined,
          search: q || undefined,
          limit: pageSize,
          offset: currentPage * pageSize,
        },
      }),
    refetchInterval: 10000, // Reduced from 5s to 10s
    staleTime: 2000, // Data stays fresh for 2s, prevents refetch on quick navigation back
    refetchIntervalInBackground: false, // Pause polling when tab is hidden
    placeholderData: keepPreviousData,
  });

  const pipelines: DagSummary[] = pipelinesData?.dags || [];
  const totalPipelines = pipelinesData?.total || 0;

  const pageIds = useMemo(
    () => pipelines.map((pipeline) => pipeline.id),
    [pipelines],
  );

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
    scope: [currentPage, pageSize, q ?? '', selectedStatus ?? ''].join('|'),
  });

  // Cancel and retry each accept only a subset of statuses, so the bar reports
  // how many of the selection they will touch instead of sending IDs the
  // backend is certain to reject. Delete accepts every status.
  const selectionCounts = useMemo(() => {
    let cancellable = 0;
    let retryable = 0;
    for (const pipeline of pipelines) {
      if (!selectedIds.has(pipeline.id)) continue;
      if (pipeline.status === 'PENDING' || pipeline.status === 'PROCESSING') {
        cancellable += 1;
      } else if (
        pipeline.status === 'FAILED' ||
        pipeline.status === 'CANCELLED'
      ) {
        retryable += 1;
      }
    }
    return { cancellable, retryable };
  }, [pipelines, selectedIds]);

  const handlePageIndexChange = useCallback(
    (index: number) => updateSearch({ page: index || undefined }),
    [updateSearch],
  );

  const cancelPipelineMutation = useMutation({
    mutationFn: (pipelineId: string) => cancelDag({ data: pipelineId }),
    onSuccess: (result) => {
      toast.success(
        i18n._(
          msg`${plural(result.cancelled_steps, {
            one: 'Cancelled # step in pipeline',
            other: 'Cancelled # steps in pipeline',
          })}`,
        ),
      );
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.dags.lists(),
      });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.stats(),
      });
    },
    onError: (error) => {
      // A pipeline that finished between render and click is not a failure —
      // refresh so the card shows its real status.
      if (isDagAlreadyTerminalError(error)) {
        toast.info(i18n._(msg`Pipeline is already completed or cancelled`));
        void queryClient.invalidateQueries({
          queryKey: queryKeys.pipeline.dags.lists(),
        });
      } else {
        toast.error(i18n._(msg`Failed to cancel pipeline`));
      }
    },
  });

  const deletePipelineMutation = useMutation({
    mutationFn: (pipelineId: string) => deletePipeline({ data: pipelineId }),
    onSuccess: () => {
      toast.success(i18n._(msg`Pipeline deleted successfully`));
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.dags.lists(),
      });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.stats(),
      });
    },
    onError: () => toast.error(i18n._(msg`Failed to delete pipeline`)),
  });

  const retryAllFailedMutation = useMutation({
    mutationFn: () => retryAllFailedPipelines(),
    onSuccess: (result) => {
      if (result.count === 0) {
        toast.info(i18n._(msg`No pipelines were retried`));
      } else {
        toast.success(
          i18n._(
            msg`${plural(result.count, {
              one: 'Retried # pipeline',
              other: 'Retried # pipelines',
            })}`,
          ),
        );
      }
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.dags.lists(),
      });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.stats(),
      });
    },
    onError: () =>
      toast.error(i18n._(msg`Failed to retry all failed pipelines`)),
  });

  const batchMutation = useBatchActionMutation({
    mutationFn: ({ ids, action }: { ids: string[]; action: BatchDagAction }) =>
      batchPipelines({ data: { ids, action } }),
    invalidate: () => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.dags.lists(),
      });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.stats(),
      });
    },
    selection: { setSelectedIds, exitSelectionMode },
    successMessage: (succeeded) =>
      i18n._(
        msg`${plural(succeeded, {
          one: 'Processed # pipeline',
          other: 'Processed # pipelines',
        })}`,
      ),
    partialMessage: (succeeded, failed) =>
      i18n._(
        msg`${plural(succeeded, {
          one: 'Processed # pipeline',
          other: 'Processed # pipelines',
        })}; ${failed} failed`,
      ),
    errorMessage: i18n._(msg`Failed to process selected pipelines`),
  });

  const handleBatchAction = useCallback(
    (action: BatchDagAction) => {
      if (selectedIds.size === 0 || batchMutation.isPending) return;

      // Cancel and retry are status-gated; sending the whole selection would
      // produce guaranteed per-item failures for the ineligible pipelines.
      let ids = Array.from(selectedIds);
      if (action.type === 'cancel' || action.type === 'retry') {
        const eligible =
          action.type === 'cancel'
            ? new Set(['PENDING', 'PROCESSING'])
            : new Set(['FAILED', 'CANCELLED']);
        const eligibleIds = new Set(
          pipelines
            .filter((pipeline) => eligible.has(pipeline.status))
            .map((pipeline) => pipeline.id),
        );
        ids = ids.filter((id) => eligibleIds.has(id));
      }

      if (ids.length === 0) return;
      batchMutation.mutate({ ids, action });
    },
    [selectedIds, batchMutation, pipelines],
  );

  const handleViewDetails = useCallback(
    (pipelineId: string) => {
      void navigate({
        to: '/pipeline/executions/$pipelineId',
        params: { pipelineId },
      });
    },
    [navigate],
  );

  // Memoize mutation callbacks to prevent child re-renders. They depend on
  // `mutate`, which is stable, rather than the mutation result object, which
  // is new on every render.
  const cancelPipelineMutate = cancelPipelineMutation.mutate;
  const deletePipelineMutate = deletePipelineMutation.mutate;

  const handleCancelPipeline = useCallback(
    (pipelineId: string) => cancelPipelineMutate(pipelineId),
    [cancelPipelineMutate],
  );

  const handleDeletePipeline = useCallback(
    (pipelineId: string) => deletePipelineMutate(pipelineId),
    [deletePipelineMutate],
  );

  const handleRetryAllFailed = useCallback(
    () => retryAllFailedMutation.mutate(),
    [retryAllFailedMutation],
  );

  return (
    <div className="min-h-screen space-y-6">
      <DashboardHeader
        icon={ListTodo}
        title={<Trans>Pipeline Jobs</Trans>}
        subtitle={<Trans>Monitor and manage processing jobs</Trans>}
        actions={
          <div className="flex flex-col sm:flex-row sm:flex-wrap items-stretch sm:items-center gap-2 w-full sm:w-auto">
            <SearchInput
              defaultValue={q ?? ''}
              onSearch={(value) =>
                updateSearch({ q: value || undefined, page: undefined })
              }
              placeholder={i18n._(msg`Search jobs...`)}
              className="flex-1 sm:w-64 min-w-[200px]"
            />
            {/* Scrolls rather than overflowing the page: the count badge grows
                with the pipeline total and the buttons keep their labels. */}
            <div className="flex items-center gap-2 shrink-0 overflow-x-auto no-scrollbar">
              <Badge
                variant="secondary"
                className="h-9 px-3 text-sm whitespace-nowrap bg-muted/50 text-muted-foreground border-border/50"
              >
                {i18n._(
                  msg`${plural(totalPipelines, {
                    one: '# pipeline',
                    other: '# pipelines',
                  })}`,
                )}
              </Badge>
              <SelectModeButton
                selectionMode={selectionMode}
                onToggle={toggleSelectionMode}
                label={i18n._(msg`Select pipelines`)}
              />
              <Button
                className="h-9 gap-2 whitespace-nowrap"
                variant="default"
                onClick={() => navigate({ to: '/pipeline/jobs/new' })}
              >
                <Plus className="h-4 w-4" />
                <span className="hidden xs:inline">
                  <Trans>Create Pipeline</Trans>
                </span>
                <span className="xs:hidden">
                  <Trans>Create</Trans>
                </span>
              </Button>
            </div>
          </div>
        }
        // The stat cards' rings and shadows reach past the row's top edge, which a
        // scrolling row would clip; the status strip below scrolls on its own.
        childrenClassName="overflow-visible"
      >
        <div className="pb-4">
          <div className="grid gap-3 grid-cols-[repeat(auto-fit,minmax(min(100%,9.5rem),1fr))]">
            <StatCard
              title={<Trans>Pending</Trans>}
              value={stats?.pending_count}
              loading={isStatsLoading}
              icon={<Clock className="h-4 w-4 text-muted-foreground" />}
            />
            <StatCard
              title={<Trans>Processing</Trans>}
              value={stats?.processing_count}
              loading={isStatsLoading}
              color="text-blue-500"
              icon={<Activity className="h-4 w-4 text-blue-500" />}
            />
            <StatCard
              title={<Trans>Completed</Trans>}
              value={stats?.completed_count}
              loading={isStatsLoading}
              color="text-green-500"
              icon={<CheckCircle2 className="h-4 w-4 text-green-500" />}
            />
            <StatCard
              title={<Trans>Failed</Trans>}
              value={stats?.failed_count}
              loading={isStatsLoading}
              color="text-red-500"
              icon={<XCircle className="h-4 w-4 text-red-500" />}
            />
            <StatCard
              title={<Trans>Avg. Duration</Trans>}
              value={stats?.avg_duration_secs}
              loading={isStatsLoading}
              color="text-purple-500"
              icon={<Timer className="h-4 w-4 text-purple-500" />}
              formatValue={formatDuration}
            />
          </div>
        </div>

        {/* Status Filter */}
        <div className="-mx-4 md:mx-0">
          <nav className="flex items-center gap-1.5 px-4 md:px-0 overflow-x-auto no-scrollbar pt-1 pb-1 flex-1">
            {STATUS_FILTERS.map(({ value, label, icon }) => (
              <FilterChip
                key={label}
                icon={icon}
                label={label}
                isActive={selectedStatus === value}
                onClick={() => handleStatusChange(value)}
              />
            ))}
          </nav>
        </div>
      </DashboardHeader>

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isError && !pipelinesData ? (
            <QueryError
              key="error"
              title={<Trans>Failed to load jobs</Trans>}
              error={error}
              onRetry={() => void refetch()}
            />
          ) : isPipelinesLoading ? (
            <LoadingSkeleton />
          ) : pipelines.length > 0 ? (
            <motion.div
              key="list"
              className={cn(CARD_GRID, 'gap-6')}
              variants={containerVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              {pipelines.map((pipeline) => (
                <motion.div key={pipeline.id} variants={itemVariants}>
                  <PipelineSummaryCard
                    pipeline={pipeline}
                    onCancelPipeline={handleCancelPipeline}
                    onDeletePipeline={handleDeletePipeline}
                    onViewDetails={handleViewDetails}
                    selectionMode={selectionMode}
                    isSelected={selectedIds.has(pipeline.id)}
                    onSelectChange={handleSelectionChange}
                  />
                </motion.div>
              ))}
            </motion.div>
          ) : (
            <PageEmptyState
              key="empty"
              icon={ListTodo}
              title={
                q || selectedStatus ? (
                  <Trans>No jobs found</Trans>
                ) : (
                  <Trans>No pipeline jobs yet</Trans>
                )
              }
              help={
                q || selectedStatus ? (
                  <Trans>Try adjusting your search or filters.</Trans>
                ) : (
                  <Trans>
                    Jobs will appear here when recordings are processed.
                  </Trans>
                )
              }
            />
          )}
        </AnimatePresence>

        <ListPagination
          pageIndex={currentPage}
          totalCount={pipelinesData?.total}
          pageSize={pageSize}
          onPageIndexChange={handlePageIndexChange}
          onPageSizeChange={(size) => updateSearch({ size, page: undefined })}
        />
      </div>
      {/* Retry All Failed FAB. Hidden while selecting so it cannot overlap the
          batch action bar, which offers a scoped retry instead. */}
      <AnimatePresence>
        {!selectionMode &&
          selectedStatus === 'FAILED' &&
          stats &&
          stats.failed_count > 0 && (
            <motion.div
              initial={{ opacity: 0, y: 50, scale: 0.9 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 50, scale: 0.9 }}
              className={cn(FAB_ANCHOR, 'md:bottom-10 md:right-10')}
            >
              <Button
                size="lg"
                onClick={handleRetryAllFailed}
                disabled={retryAllFailedMutation.isPending}
                className="rounded-full shadow-xl shadow-red-500/30 gap-2.5 h-14 px-7 bg-red-600 hover:bg-red-700 text-white border-none group transition-all duration-300 active:scale-95"
              >
                <RefreshCw
                  className={cn(
                    'h-5 w-5',
                    retryAllFailedMutation.isPending && 'animate-spin',
                    !retryAllFailedMutation.isPending &&
                      'group-hover:rotate-180 transition-transform duration-500',
                  )}
                />
                <span className="font-bold tracking-tight text-base">
                  <Trans>Retry All Failed</Trans>
                </span>
              </Button>
            </motion.div>
          )}
      </AnimatePresence>

      <AnimatePresence>
        {selectionMode && (
          <PipelineBatchActionBar
            selectedCount={selectedIds.size}
            pageCount={pipelines.length}
            allPageSelected={allPageSelected}
            cancellableCount={selectionCounts.cancellable}
            retryableCount={selectionCounts.retryable}
            runningCount={selectionCounts.cancellable}
            isPending={batchMutation.isPending}
            onSelectPage={selectPage}
            onClearSelection={clearSelection}
            onAction={handleBatchAction}
            onExit={toggleSelectionMode}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

const StatCard = memo(function StatCard({
  title,
  value,
  loading,
  color,
  icon,
  formatValue,
}: {
  title: React.ReactNode;
  value?: number | null;
  loading?: boolean;
  color?: string;
  icon?: React.ReactNode;
  formatValue?: (v: number) => string;
}) {
  return (
    <Card className="flex flex-col p-4 space-y-2 ring-1 ring-border/50 shadow-sm hover:shadow-md transition-shadow duration-200">
      <div className="flex items-center justify-between text-xs sm:text-sm font-medium text-muted-foreground">
        {title}
        {icon}
      </div>
      <div
        className={cn('text-xl sm:text-2xl font-bold tracking-tight', color)}
      >
        {loading ? (
          <Skeleton className="h-8 w-16" />
        ) : formatValue ? (
          formatValue(value || 0)
        ) : (
          value || 0
        )}
      </div>
    </Card>
  );
});

// Memoized loading skeleton to prevent re-creation
const SKELETON_ITEMS = [1, 2, 3, 4, 5, 6];

const LoadingSkeleton = memo(function LoadingSkeleton() {
  return (
    <motion.div
      key="loading"
      initial={false}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0, transition: { duration: 0.1 } }}
      className={cn(CARD_GRID, 'gap-6')}
    >
      {SKELETON_ITEMS.map((i) => (
        <div
          key={i}
          className="border rounded-xl bg-muted/10 animate-pulse flex flex-col shadow-sm overflow-hidden"
        >
          {/* CardHeader */}
          <div className="flex items-center gap-4 p-4 sm:p-5 pb-3">
            <Skeleton className="h-11 w-11 rounded-2xl shrink-0" />
            <div className="flex-1 min-w-0 space-y-2.5">
              <Skeleton className="h-4 w-2/3" />
              <Skeleton className="h-3 w-1/3" />
            </div>
            <Skeleton className="h-6 w-16 rounded-full shrink-0" />
          </div>
          {/* CardContent */}
          <div className="flex-1 px-6 pb-6 pt-1 space-y-5">
            <Skeleton className="h-3 w-3/5" />
            <div className="flex items-center gap-3">
              <Skeleton className="h-7 w-18 rounded-md" />
              <Skeleton className="h-7 w-18 rounded-md" />
              <Skeleton className="h-7 w-18 rounded-md" />
            </div>
            <Skeleton className="h-3 w-2/5" />
            <Skeleton className="h-12 w-full rounded-md" />
          </div>
          {/* CardFooter */}
          <div className="flex justify-between items-center border-t border-border/20 px-6 py-4 bg-muted/5">
            <Skeleton className="h-3 w-16" />
            <Skeleton className="h-3 w-10" />
          </div>
        </div>
      ))}
    </motion.div>
  );
});
