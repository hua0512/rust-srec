import { useMemo, useState, useEffect } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { motion, AnimatePresence } from 'motion/react';
import {
  getPipelineStats,
  listPipelines,
  cancelPipeline,
  createPipelineJob,
} from '@/server/functions';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
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
  Workflow,
  Trash2,
  Loader2,
} from 'lucide-react';
import { toast } from 'sonner';
import { z } from 'zod';
import { PipelineSummaryCard } from '@/components/pipeline/jobs/pipeline-summary-card';
import { listJobPresets } from '@/server/functions/job';

const createPipelineSchema = z.object({
  name: z.string().min(1, 'Pipeline name is required'),
  session_id: z.string().min(1, 'Session ID is required'),
  streamer_id: z.string().min(1, 'Streamer ID is required'),
  input_path: z.string().min(1, 'Input path is required'),
  steps: z.array(z.string()).min(1, 'Add at least one step'),
});

type CreatePipelineForm = z.infer<typeof createPipelineSchema>;

// Helper function to format duration in human-readable format
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
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [selectedPreset, setSelectedPreset] = useState('');

  const createPipelineForm = useForm<CreatePipelineForm>({
    resolver: zodResolver(createPipelineSchema),
    defaultValues: {
      name: '',
      session_id: '',
      streamer_id: '',
      input_path: '',
      steps: [],
    },
  });
  const stepsValue = createPipelineForm.watch('steps');

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(searchQuery);
      setCurrentPage(0);
    }, 300);
    return () => clearTimeout(timer);
  }, [searchQuery]);

  useEffect(() => {
    if (!createDialogOpen) {
      createPipelineForm.reset({
        session_id: '',
        streamer_id: '',
        input_path: '',
        steps: [],
      });
      setSelectedPreset('');
    }
  }, [createDialogOpen, createPipelineForm]);

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

  const { data: presetsData, isLoading: presetsLoading } = useQuery({
    queryKey: ['job', 'presets'],
    queryFn: () => listJobPresets({ data: {} }),
    enabled: createDialogOpen,
  });
  const presets = presetsData?.presets ?? [];

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
    onSuccess: (result) => {
      toast.success(t`Cancelled ${result.cancelled_count} jobs in pipeline`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'pipelines'] });
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'stats'] });
    },
    onError: () => toast.error(t`Failed to cancel pipeline`),
  });

  const createPipelineMutation = useMutation({
    mutationFn: (payload: CreatePipelineForm) => {
      // Convert string steps to tagged preset format with generated IDs and sequential dependencies
      const formattedPayload = {
        session_id: payload.session_id,
        streamer_id: payload.streamer_id,
        input_path: payload.input_path,
        dag: {
          name: payload.name,
          steps: payload.steps.map((name, idx) => ({
            id: `${name}_${idx}`,
            step: {
              type: 'preset' as const,
              name,
            },
            depends_on: [], // Steps are parallel by default now
          })),
        },
      };
      return createPipelineJob({ data: formattedPayload });
    },
    onSuccess: () => {
      toast.success(t`Pipeline created`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'pipelines'] });
      setCreateDialogOpen(false);
      createPipelineForm.reset({
        session_id: '',
        streamer_id: '',
        input_path: '',
        steps: [],
      });
      setSelectedPreset('');
    },
    onError: (error: any) => {
      toast.error(error?.message || t`Failed to create pipeline`);
    },
  });

  const handleAddStep = () => {
    if (!selectedPreset) return;
    const current = createPipelineForm.getValues('steps');
    createPipelineForm.setValue('steps', [...current, selectedPreset], {
      shouldDirty: true,
    });
    setSelectedPreset('');
  };

  const handleRemoveStep = (index: number) => {
    const current = createPipelineForm.getValues('steps');
    createPipelineForm.setValue(
      'steps',
      current.filter((_, i) => i !== index),
      { shouldDirty: true },
    );
  };

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
            <div className="flex items-center gap-2 w-full md:w-auto">
              {/* Search Input */}
              <div className="relative flex-1 md:w-64">
                <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                <Input
                  placeholder={t`Search jobs...`}
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="pl-9 h-9"
                />
              </div>
              <Badge
                variant="secondary"
                className="h-9 px-3 text-sm whitespace-nowrap"
              >
                {totalPipelines} <Trans>pipelines</Trans>
              </Badge>
              <Dialog
                open={createDialogOpen}
                onOpenChange={setCreateDialogOpen}
              >
                <DialogTrigger asChild>
                  <Button className="h-9 gap-2" variant="default">
                    <Plus className="h-4 w-4" />
                    <Trans>Create Pipeline</Trans>
                  </Button>
                </DialogTrigger>
                <DialogContent className="max-w-xl">
                  <DialogHeader>
                    <DialogTitle>
                      <Trans>Create Pipeline Job</Trans>
                    </DialogTitle>
                    <DialogDescription>
                      <Trans>
                        Provide session details and choose the processing steps
                        to launch a manual pipeline.
                      </Trans>
                    </DialogDescription>
                  </DialogHeader>
                  <Form {...createPipelineForm}>
                    <form
                      className="space-y-4"
                      onSubmit={createPipelineForm.handleSubmit((values) =>
                        createPipelineMutation.mutate(values),
                      )}
                    >
                      <FormField
                        control={createPipelineForm.control}
                        name="name"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel>
                              <Trans>Pipeline Name</Trans>
                            </FormLabel>
                            <FormControl>
                              <Input placeholder="My Archiving Workflow" {...field} />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <FormField
                        control={createPipelineForm.control}
                        name="session_id"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel>
                              <Trans>Session ID</Trans>
                            </FormLabel>
                            <FormControl>
                              <Input placeholder="session-uuid" {...field} />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <FormField
                        control={createPipelineForm.control}
                        name="streamer_id"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel>
                              <Trans>Streamer ID</Trans>
                            </FormLabel>
                            <FormControl>
                              <Input placeholder="streamer-uuid" {...field} />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <FormField
                        control={createPipelineForm.control}
                        name="input_path"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel>
                              <Trans>Input Path</Trans>
                            </FormLabel>
                            <FormControl>
                              <Input
                                placeholder="C:\path\to\recording.flv"
                                {...field}
                              />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <FormField
                        control={createPipelineForm.control}
                        name="steps"
                        render={() => (
                          <FormItem>
                            <FormLabel>
                              <Trans>Pipeline Steps</Trans>
                            </FormLabel>
                            <div className="space-y-3">
                              <div className="flex gap-2">
                                <Select
                                  value={selectedPreset}
                                  onValueChange={setSelectedPreset}
                                  disabled={presetsLoading}
                                >
                                  <SelectTrigger>
                                    <SelectValue
                                      placeholder={t`Select preset`}
                                    />
                                  </SelectTrigger>
                                  <SelectContent>
                                    {presets.map((preset) => (
                                      <SelectItem
                                        key={preset.id}
                                        value={preset.name}
                                      >
                                        {preset.name}
                                      </SelectItem>
                                    ))}
                                  </SelectContent>
                                </Select>
                                <Button
                                  type="button"
                                  onClick={handleAddStep}
                                  disabled={!selectedPreset}
                                  size="icon"
                                >
                                  <Plus className="h-4 w-4" />
                                </Button>
                              </div>
                              {stepsValue.length > 0 ? (
                                <div className="space-y-2 border rounded-md p-3">
                                  {stepsValue.map((step, idx) => (
                                    <div
                                      key={`${step}-${idx}`}
                                      className="flex items-center justify-between text-sm bg-muted/40 px-3 py-2 rounded-md"
                                    >
                                      <div className="flex items-center gap-2">
                                        <Workflow className="h-4 w-4 text-muted-foreground" />
                                        <span>
                                          {idx + 1}. {step}
                                        </span>
                                      </div>
                                      <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        className="h-8 w-8 text-muted-foreground hover:text-destructive"
                                        onClick={() => handleRemoveStep(idx)}
                                      >
                                        <Trash2 className="h-4 w-4" />
                                      </Button>
                                    </div>
                                  ))}
                                </div>
                              ) : (
                                <div className="text-sm text-muted-foreground border border-dashed rounded-md p-4 text-center">
                                  <Trans>No steps selected yet</Trans>
                                </div>
                              )}
                            </div>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <DialogFooter>
                        <Button
                          type="button"
                          variant="outline"
                          onClick={() => setCreateDialogOpen(false)}
                        >
                          <Trans>Cancel</Trans>
                        </Button>
                        <Button
                          type="submit"
                          disabled={createPipelineMutation.isPending}
                        >
                          {createPipelineMutation.isPending ? (
                            <span className="flex items-center gap-2">
                              <Loader2 className="h-4 w-4 animate-spin" />
                              <Trans>Creating...</Trans>
                            </span>
                          ) : (
                            <Trans>Create</Trans>
                          )}
                        </Button>
                      </DialogFooter>
                    </form>
                  </Form>
                </DialogContent>
              </Dialog>
            </div>
          </div>

          {/* Stats Overview */}
          <div className="px-4 md:px-8 pb-4">
            <div className="grid gap-3 grid-cols-2 md:grid-cols-5">
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
          <div className="px-4 md:px-8 pb-3 overflow-x-auto no-scrollbar">
            <nav className="flex items-center gap-1">
              {STATUS_FILTERS.map(({ value, label, icon: Icon }) => (
                <button
                  key={label}
                  onClick={() => handleStatusChange(value)}
                  className={`relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 flex items-center gap-1.5 ${selectedStatus === value
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted'
                    }`}
                >
                  <Icon className="h-3.5 w-3.5" />
                  <span className="relative z-10">{label}</span>
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
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6"
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
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.3 }}
            >
              {pipelines.map((pipeline, index) => (
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
  loading: boolean;
  color?: string;
  icon?: React.ReactNode;
  formatValue?: (v: number | null | undefined) => string;
}) {
  return (
    <Card className="bg-background/50 backdrop-blur-sm border-border/40">
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-1 pt-3 px-4">
        <CardTitle className="text-xs font-medium text-muted-foreground">
          {title}
        </CardTitle>
        {icon}
      </CardHeader>
      <CardContent className="px-4 pb-3">
        {loading ? (
          <Skeleton className="h-7 w-12" />
        ) : (
          <div className={`text-xl font-bold ${color}`}>
            {formatValue ? formatValue(value) : (value ?? 0)}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
