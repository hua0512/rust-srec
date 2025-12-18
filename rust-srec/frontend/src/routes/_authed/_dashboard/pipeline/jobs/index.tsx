import { useMemo, useState, useEffect } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import {
  getPipelineStats,
  listPipelines,
  cancelPipeline,
  deletePipeline,
} from '@/server/functions';
import { Card } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
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
  RefreshCw,
  XCircle,
  CheckCircle2,
  Clock,
  AlertCircle,
  Timer,
  ListTodo,
  Search,
  Plus,
} from 'lucide-react';
import { toast } from 'sonner';
import { PipelineSummaryCard } from '@/components/pipeline/jobs/pipeline-summary-card';
import { formatDuration } from '@/lib/format';

export const Route = createFileRoute('/_authed/_dashboard/pipeline/jobs/')({
  component: PipelineJobsPage,
});

const PAGE_SIZES = [12, 24, 48, 96];

function PipelineJobsPage() {
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const [selectedStatus, setSelectedStatus] = useState<string | null>(null);

  const STATUS_FILTERS = [
    { value: null, label: t`All`, icon: ListTodo },
    { value: 'PENDING', label: t`Pending`, icon: Clock },
    { value: 'PROCESSING', label: t`Processing`, icon: RefreshCw },
    { value: 'COMPLETED', label: t`Completed`, icon: CheckCircle2 },
    { value: 'FAILED', label: t`Failed`, icon: XCircle },
  ] as const;

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

  // Reset page when status changes
  const handleStatusChange = (status: string | null) => {
    setSelectedStatus(status);
    setCurrentPage(0);
  };

  const { data: stats, isLoading: isStatsLoading } = useQuery({
    queryKey: ['pipeline', 'stats'],
    queryFn: () => getPipelineStats(),
    refetchInterval: 5000,
  });

  const {
    data: pipelinesData,
    isLoading: isPipelinesLoading,
    isError,
    error,
  } = useQuery({
    queryKey: [
      'pipeline',
      'pipelines',
      selectedStatus,
      debouncedSearch,
      pageSize,
      currentPage,
    ],
    queryFn: () =>
      listPipelines({
        data: {
          status: selectedStatus || undefined,
          search: debouncedSearch || undefined,
          limit: pageSize,
          offset: currentPage * pageSize,
        },
      }),
    refetchInterval: 5000,
    placeholderData: keepPreviousData,
  });

  const pipelines = pipelinesData?.dags || [];
  const totalPipelines = pipelinesData?.total || 0;

  const totalPages = Math.ceil(totalPipelines / pageSize);

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

  const cancelPipelineMutation = useMutation({
    mutationFn: (pipelineId: string) => cancelPipeline({ data: pipelineId }),
    onSuccess: (result: any) => {
      toast.success(t`Cancelled ${result.cancelled_steps} steps in pipeline`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'pipelines'] });
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'stats'] });
    },
    onError: (error: any) => {
      // Handle case where DAG is already in terminal state (completed/failed)
      if (error?.body?.message?.includes('terminal state')) {
        toast.info(t`Pipeline is already completed or cancelled`);
        queryClient.invalidateQueries({ queryKey: ['pipeline', 'pipelines'] });
      } else {
        toast.error(t`Failed to cancel pipeline`);
      }
    },
  });

  const deletePipelineMutation = useMutation({
    mutationFn: (pipelineId: string) => deletePipeline({ data: pipelineId }),
    onSuccess: () => {
      toast.success(t`Pipeline deleted successfully`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'pipelines'] });
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'stats'] });
    },
    onError: () => toast.error(t`Failed to delete pipeline`),
  });

  const handleViewDetails = (pipelineId: string) => {
    navigate({
      to: '/pipeline/executions/$pipelineId',
      params: { pipelineId },
    });
  };

  if (isError) {
    return (
      <div className="space-y-8 p-6 md:p-10 max-w-7xl mx-auto">
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertTitle>
            <Trans>Error</Trans>
          </AlertTitle>
          <AlertDescription>
            <Trans>Failed to load jobs: {error.message}</Trans>
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <div className="min-h-screen space-y-6">
      {/* Header */}
      <div className="border-b border-border/40">
        <div className="w-full">
          {/* Title Row */}
          <div className="flex flex-col md:flex-row gap-4 items-start md:items-center justify-between p-4 md:px-8">
            <div className="flex items-center gap-4">
              <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
                <ListTodo className="h-6 w-6 text-primary" />
              </div>
              <div>
                <h1 className="text-xl font-semibold tracking-tight">
                  <Trans>Pipeline Jobs</Trans>
                </h1>
                <p className="text-sm text-muted-foreground">
                  <Trans>Monitor and manage processing jobs</Trans>
                </p>
              </div>
            </div>
            <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2 w-full md:w-auto">
              {/* Search Input */}
              <div className="relative flex-1 sm:w-64">
                <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                <Input
                  placeholder={t`Search jobs...`}
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="pl-9 h-9"
                />
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <Badge
                  variant="secondary"
                  className="h-9 px-3 text-sm whitespace-nowrap bg-muted/50 text-muted-foreground border-border/50"
                >
                  {totalPipelines} <Trans>pipelines</Trans>
                </Badge>
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
          </div>

          {/* Stats Overview */}
          <div className="px-4 md:px-8 pb-4 w-full">
            <div className="grid gap-3 grid-cols-2 sm:grid-cols-3 lg:grid-cols-5">
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
                icon={
                  <RefreshCw className="h-4 w-4 text-blue-500 animate-spin" />
                }
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
                value={stats?.avg_processing_time_secs}
                loading={isStatsLoading}
                color="text-purple-500"
                icon={<Timer className="h-4 w-4 text-purple-500" />}
                formatValue={formatDuration}
              />
            </div>
          </div>

          {/* Status Filter */}
          <div className="px-4 md:px-8 pb-3 -mx-4 md:mx-0">
            <nav className="flex items-center gap-1.5 px-4 md:px-0 overflow-x-auto no-scrollbar pt-1 pb-1">
              {STATUS_FILTERS.map(({ value, label, icon: Icon }) => (
                <button
                  key={label}
                  onClick={() => handleStatusChange(value)}
                  className={`relative px-3.5 py-1.5 text-xs sm:text-sm font-medium rounded-full transition-all duration-200 flex items-center gap-1.5 shrink-0 shadow-sm ring-1 ${
                    selectedStatus === value
                      ? 'bg-primary text-primary-foreground ring-primary'
                      : 'text-muted-foreground hover:text-foreground bg-background hover:bg-muted ring-border/50'
                  }`}
                >
                  <Icon className="h-4 w-4" />
                  <span className="relative z-10 whitespace-nowrap">
                    {label}
                  </span>
                </button>
              ))}
            </nav>
          </div>
        </div>
      </div>

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isPipelinesLoading ? (
            <motion.div
              key="loading"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 gap-6"
            >
              {[1, 2, 3, 4, 5, 6].map((i) => (
                <div
                  key={i}
                  className="h-[200px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm"
                >
                  <div className="flex justify-between items-start">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <Skeleton className="h-6 w-16" />
                  </div>
                  <div className="space-y-2 pt-2">
                    <Skeleton className="h-6 w-3/4" />
                    <Skeleton className="h-4 w-1/2" />
                  </div>
                  <div className="pt-4 mt-auto">
                    <Skeleton className="h-8 w-full rounded-md" />
                  </div>
                </div>
              ))}
            </motion.div>
          ) : pipelines.length > 0 ? (
            <motion.div
              key="list"
              className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 gap-6"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.3 }}
            >
              {pipelines.map((pipeline: any, index: number) => (
                <motion.div
                  key={pipeline.id}
                  initial={{ opacity: 0, y: 20 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{
                    duration: 0.3,
                    delay: Math.min(index * 0.05, 0.3),
                  }}
                >
                  <PipelineSummaryCard
                    pipeline={pipeline}
                    onCancelPipeline={(pipelineId) =>
                      cancelPipelineMutation.mutate(pipelineId)
                    }
                    onDeletePipeline={(pipelineId) =>
                      deletePipelineMutation.mutate(pipelineId)
                    }
                    onViewDetails={handleViewDetails}
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
                <ListTodo className="h-16 w-16 text-primary/60" />
              </div>
              <div className="space-y-2 max-w-md">
                <h3 className="font-semibold text-2xl tracking-tight">
                  {debouncedSearch || selectedStatus ? (
                    <Trans>No jobs found</Trans>
                  ) : (
                    <Trans>No pipeline jobs yet</Trans>
                  )}
                </h3>
                <p className="text-muted-foreground">
                  {debouncedSearch || selectedStatus ? (
                    <Trans>Try adjusting your search or filters.</Trans>
                  ) : (
                    <Trans>
                      Jobs will appear here when recordings are processed.
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
            <div className="flex items-center gap-2 shrink-0">
              <span className="text-xs sm:text-sm text-muted-foreground">
                <Trans>Per page:</Trans>
              </span>
              <Select
                value={pageSize.toString()}
                onValueChange={(value) => {
                  setPageSize(Number(value));
                  setCurrentPage(0);
                }}
              >
                <SelectTrigger className="w-16 sm:w-20 h-8 text-xs sm:text-sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PAGE_SIZES.map((size) => (
                    <SelectItem
                      key={size}
                      value={size.toString()}
                      className="text-xs sm:text-sm"
                    >
                      {size}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="flex-1 min-w-0">
              <Pagination className="justify-end w-auto overflow-x-auto no-scrollbar">
                <PaginationContent className="flex-nowrap">
                  <PaginationItem>
                    <PaginationPrevious
                      onClick={() =>
                        setCurrentPage((p: number) => Math.max(0, p - 1))
                      }
                      className={cn(
                        'h-8 px-2 sm:px-3 text-xs sm:text-sm',
                        currentPage === 0
                          ? 'pointer-events-none opacity-50'
                          : 'cursor-pointer',
                      )}
                    />
                  </PaginationItem>

                  <div className="hidden sm:flex items-center">
                    {paginationPages.map(
                      (page: number | 'ellipsis', idx: number) =>
                        page === 'ellipsis' ? (
                          <PaginationItem key={`ellipsis-${idx}`}>
                            <PaginationEllipsis />
                          </PaginationItem>
                        ) : (
                          <PaginationItem key={page}>
                            <PaginationLink
                              isActive={currentPage === page}
                              onClick={() => setCurrentPage(page)}
                              className="cursor-pointer h-8 w-8 text-xs font-medium"
                            >
                              {page + 1}
                            </PaginationLink>
                          </PaginationItem>
                        ),
                    )}
                  </div>

                  <div className="sm:hidden flex items-center px-4 text-xs font-medium text-muted-foreground">
                    {currentPage + 1} / {totalPages}
                  </div>

                  <PaginationItem>
                    <PaginationNext
                      onClick={() =>
                        setCurrentPage((p: number) =>
                          Math.min(totalPages - 1, p + 1),
                        )
                      }
                      className={cn(
                        'h-8 px-2 sm:px-3 text-xs sm:text-sm',
                        currentPage >= totalPages - 1
                          ? 'pointer-events-none opacity-50'
                          : 'cursor-pointer',
                      )}
                    />
                  </PaginationItem>
                </PaginationContent>
              </Pagination>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function StatCard({
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
}
