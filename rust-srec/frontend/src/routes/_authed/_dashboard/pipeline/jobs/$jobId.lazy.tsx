import React, { useEffect } from 'react';
import { Link, createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  useInfiniteQuery,
} from '@tanstack/react-query';
import {
  getPipelineJob,
  getPipelineJobLogs,
  getPipelineJobProgress,
  retryPipelineJob,
  cancelPipelineJob,
} from '@/server/functions/pipeline';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';

import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg, t, plural } from '@lingui/core/macro';
import { toast } from 'sonner';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { motion } from 'motion/react';
import { useInView } from '@/lib/hooks/use-in-view';
import { cn } from '@/lib/utils';
import {
  ArrowLeft,
  AlertCircle,
  Terminal,
  RotateCcw,
  XCircle,
  CheckCircle2,
  Clock,
  Workflow,
  Box,
  Cpu,
  Calendar,
  RefreshCw,
  StopCircle,
  FileCode,
  FileOutput,
  HardDrive,
  Hash,
  Trash2,
} from 'lucide-react';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/jobs/$jobId',
)({
  component: JobDetailsPage,
});

import { formatDuration } from '@/lib/format';

const STATUS_CONFIG: Record<
  string,
  {
    icon: React.ElementType;
    color: string;
    badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline';
    animate?: boolean;
    gradient: string;
    borderColor: string;
  }
> = {
  PENDING: {
    icon: Clock,
    color: 'text-muted-foreground',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
    borderColor: 'border-gray-500/20',
  },
  PROCESSING: {
    icon: RefreshCw,
    color: 'text-blue-500',
    badgeVariant: 'default',
    animate: true,
    gradient: 'from-blue-500/20 to-blue-500/5',
    borderColor: 'border-blue-500/20',
  },
  COMPLETED: {
    icon: CheckCircle2,
    color: 'text-emerald-500',
    badgeVariant: 'secondary',
    gradient: 'from-emerald-500/20 to-emerald-500/5',
    borderColor: 'border-emerald-500/20',
  },
  FAILED: {
    icon: XCircle,
    color: 'text-red-500',
    badgeVariant: 'destructive',
    gradient: 'from-red-500/20 to-red-500/5',
    borderColor: 'border-red-500/20',
  },
  CANCELLED: {
    icon: AlertCircle,
    color: 'text-gray-500',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
    borderColor: 'border-gray-500/20',
  },
  INTERRUPTED: {
    icon: AlertCircle,
    color: 'text-orange-500',
    badgeVariant: 'secondary',
    gradient: 'from-orange-500/20 to-orange-500/5',
    borderColor: 'border-orange-500/20',
  },
};

function JobDetailsPage() {
  const { jobId } = Route.useParams();
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  const {
    data: job,
    isLoading,
    isError,
    error,
  } = useQuery({
    queryKey: ['pipeline', 'job', jobId],
    queryFn: () => getPipelineJob({ data: jobId }),
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return status === 'PROCESSING' || status === 'PENDING' ? 1000 : false;
    },
  });

  const { data: progressSnapshot } = useQuery({
    queryKey: ['pipeline', 'job', jobId, 'progress'],
    queryFn: () => getPipelineJobProgress({ data: { id: jobId } }),
    enabled: job?.status === 'PROCESSING',
    refetchInterval: 1000,
    retry: false, // Don't retry on 404 when no progress is available
    throwOnError: false, // Silently handle errors - progress is optional
  });

  // Fetch logs separately with infinite scroll
  const {
    data: logsData,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
    isLoading: isLogsLoading,
  } = useInfiniteQuery({
    queryKey: ['pipeline', 'job', jobId, 'logs'],
    queryFn: ({ pageParam }) =>
      getPipelineJobLogs({
        data: { id: jobId, limit: 1000, offset: pageParam },
      }),
    initialPageParam: 0,
    getNextPageParam: (lastPage) => {
      const nextOffset = lastPage.offset + lastPage.limit;
      return nextOffset < lastPage.total ? nextOffset : undefined;
    },
    refetchInterval: () => {
      const status = job?.status;
      return status === 'PROCESSING' || status === 'PENDING' ? 2000 : false;
    },
    enabled: !!job,
  });

  const logs = logsData?.pages.flatMap((page) => page.items) || [];
  const totalLogs = logsData?.pages[0]?.total || 0;

  const { ref: loadMoreRef, inView } = useInView();

  useEffect(() => {
    if (inView && hasNextPage) {
      void fetchNextPage();
    }
  }, [inView, hasNextPage, fetchNextPage]);

  const retryMutation = useMutation({
    mutationFn: (id: string) => retryPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Job retry initiated`));
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'job', jobId],
      });
    },
    onError: () => toast.error(i18n._(msg`Failed to retry job`)),
  });

  const cancelMutation = useMutation({
    mutationFn: (id: string) => cancelPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Job cancelled`));
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'job', jobId],
      });
    },
    onError: () => toast.error(i18n._(msg`Failed to cancel job`)),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => cancelPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Job deleted`));
      // Redirect to jobs list on deletion since the job no longer exists
      window.history.back();
    },
    onError: () => toast.error(i18n._(msg`Failed to delete job`)),
  });

  if (isLoading) {
    return (
      <div className="min-h-screen p-6 md:p-10 max-w-7xl mx-auto space-y-8 bg-background">
        <div className="flex items-center gap-6">
          <Skeleton className="h-12 w-12 rounded-xl" />
          <div className="space-y-2">
            <Skeleton className="h-8 w-64" />
            <Skeleton className="h-4 w-32" />
          </div>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
          <Skeleton className="h-[200px] md:col-span-2 rounded-2xl" />
          <Skeleton className="h-[200px] rounded-2xl" />
        </div>
        <Skeleton className="h-[400px] rounded-2xl" />
      </div>
    );
  }

  if (isError || !job) {
    return (
      <div className="min-h-screen flex items-center justify-center p-6">
        <Card className="max-w-md w-full border-destructive/20 bg-destructive/5 backdrop-blur-sm">
          <CardHeader className="text-center pb-2">
            <div className="mx-auto p-3 rounded-full bg-destructive/10 w-fit mb-2">
              <AlertCircle className="h-6 w-6 text-destructive" />
            </div>
            <CardTitle className="text-xl text-destructive">
              <Trans>Error Loading Job</Trans>
            </CardTitle>
            <CardDescription>
              {error?.message || i18n._(msg`Job not found`)}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex justify-center pb-6">
            <Button variant="outline" onClick={() => window.history.back()}>
              <ArrowLeft className="mr-2 h-4 w-4" /> <Trans>Go Back</Trans>
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  const statusConfig = STATUS_CONFIG[job.status] || STATUS_CONFIG.PENDING;
  const StatusIcon = statusConfig.icon;

  return (
    <div className="relative min-h-screen bg-background overflow-hidden selection:bg-primary/20">
      {/* Ambient Background */}
      <div className="fixed inset-0 pointer-events-none">
        <div className="absolute top-0 left-0 -mt-20 -ml-20 w-[500px] h-[500px] bg-primary/5 rounded-full blur-[120px]" />
        <div className="absolute bottom-0 right-0 -mb-40 -mr-20 w-[600px] h-[600px] bg-blue-500/5 rounded-full blur-[120px]" />
      </div>
      <div className="relative z-10 max-w-7xl mx-auto px-6 py-8 pb-32">
        {/* Header */}
        <div className="flex flex-col gap-8 mb-10">
          <motion.div
            initial={{ opacity: 0, x: -20 }}
            animate={{ opacity: 1, x: 0 }}
          >
            <Button
              variant="ghost"
              size="sm"
              asChild
              className="group text-muted-foreground hover:text-foreground hover:bg-transparent px-0"
            >
              <Link to="/pipeline/jobs" className="flex items-center">
                <ArrowLeft className="mr-2 h-4 w-4 transition-transform group-hover:-translate-x-1" />
                <Trans>Back to Jobs</Trans>
              </Link>
            </Button>
          </motion.div>

          <div className="flex flex-col md:flex-row md:items-start justify-between gap-6">
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.1 }}
              className="flex items-start gap-5"
            >
              <div
                className={cn(
                  'flex items-center justify-center w-16 h-16 rounded-2xl shadow-lg ring-1 ring-white/10 backdrop-blur-md bg-gradient-to-br shrink-0',
                  statusConfig.gradient,
                )}
              >
                <StatusIcon
                  className={cn(
                    'h-8 w-8',
                    statusConfig.color,
                    statusConfig.animate && 'animate-spin',
                  )}
                />
              </div>
              <div>
                <div className="flex items-center gap-3 mb-1.5">
                  <h1 className="text-3xl font-bold tracking-tight text-foreground">
                    <Trans>Job Details</Trans>
                  </h1>
                  <Badge
                    variant="outline"
                    className={cn(
                      'border bg-background/50 backdrop-blur font-mono text-xs uppercase tracking-wider h-6',
                      statusConfig.borderColor,
                      statusConfig.color,
                    )}
                  >
                    {i18n._(
                      job.status === 'PENDING'
                        ? msg`Pending`
                        : job.status === 'PROCESSING'
                          ? msg`Processing`
                          : job.status === 'COMPLETED'
                            ? msg`Completed`
                            : job.status === 'FAILED'
                              ? msg`Failed`
                              : job.status === 'INTERRUPTED'
                                ? msg`Interrupted`
                                : job.status,
                    )}
                  </Badge>
                </div>
                <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground font-medium">
                  <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-muted/50 border border-border/50">
                    <Workflow className="h-3.5 w-3.5" />
                    {(() => {
                      const def = getProcessorDefinition(job.processor_type);
                      return def
                        ? i18n._(def.label)
                        : job.processor_type.replace(/_/g, ' ');
                    })()}
                  </span>
                  <span className="opacity-40">•</span>
                  <span className="font-mono text-xs opacity-70 selection:bg-muted select-all">
                    ID: {job.id}
                  </span>
                </div>
              </div>
            </motion.div>

            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.2 }}
              className="flex items-center gap-3"
            >
              {job.status === 'FAILED' && (
                <Button
                  className="bg-primary shadow-lg shadow-primary/20 hover:shadow-primary/40 transition-all font-medium"
                  onClick={() => retryMutation.mutate(job.id)}
                  disabled={retryMutation.isPending}
                >
                  <RotateCcw
                    className={cn(
                      'mr-2 h-4 w-4',
                      retryMutation.isPending && 'animate-spin',
                    )}
                  />
                  <Trans>Retry Job</Trans>
                </Button>
              )}
              {['PENDING', 'PROCESSING'].includes(job.status) && (
                <Button
                  variant="destructive"
                  className="shadow-lg shadow-destructive/20 hover:shadow-destructive/40 transition-all"
                  onClick={() => cancelMutation.mutate(job.id)}
                  disabled={cancelMutation.isPending}
                >
                  <StopCircle className="mr-2 h-4 w-4" />{' '}
                  <Trans>Cancel Execution</Trans>
                </Button>
              )}
              {['COMPLETED', 'FAILED', 'INTERRUPTED'].includes(job.status) && (
                <Button
                  variant="destructive"
                  className="shadow-lg shadow-destructive/20 hover:shadow-destructive/40 transition-all"
                  onClick={() => {
                    if (
                      confirm(
                        i18n._(msg`Are you sure you want to delete this job?`),
                      )
                    ) {
                      deleteMutation.mutate(job.id);
                    }
                  }}
                  disabled={deleteMutation.isPending}
                >
                  <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete Job</Trans>
                </Button>
              )}
            </motion.div>
          </div>
        </div>

        {/* Content Grid */}
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
          {/* Left Column: Metadata & Config */}
          <div className="space-y-6 lg:col-span-2">
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.3 }}
            >
              <Card className="bg-card/40 backdrop-blur-sm border-border/40 shadow-sm">
                <CardHeader className="border-b border-border/40 pb-4">
                  <CardTitle className="text-lg font-semibold flex items-center gap-2">
                    <Box className="h-5 w-5 text-primary/70" />
                    <Trans>Context</Trans>
                  </CardTitle>
                </CardHeader>
                <CardContent className="p-6 grid md:grid-cols-2 gap-8">
                  <InfoGroup
                    label={i18n._(msg`Session ID`)}
                    value={job.session_id}
                    icon={<Hash className="h-4 w-4" />}
                    mono
                  />
                  <InfoGroup
                    label={i18n._(msg`Streamer ID`)}
                    value={job.streamer_id}
                    icon={<Cpu className="h-4 w-4" />}
                    mono
                  />
                  {job.pipeline_id && (
                    <InfoGroup
                      label={i18n._(msg`Pipeline`)}
                      value={
                        <Link
                          to="/pipeline/executions/$pipelineId"
                          params={{ pipelineId: job.pipeline_id }}
                          className="text-primary hover:underline underline-offset-4 decoration-primary/30"
                        >
                          PIPE-{job.pipeline_id.substring(0, 8)}
                        </Link>
                      }
                      icon={<Workflow className="h-4 w-4" />}
                    />
                  )}
                  <InfoGroup
                    label={i18n._(msg`Resources`)}
                    value={
                      <div className="flex gap-2">
                        {job.execution_info?.input_size_bytes && (
                          <Badge variant="secondary" className="text-[10px]">
                            In:{' '}
                            {(
                              job.execution_info.input_size_bytes /
                              1024 /
                              1024
                            ).toFixed(1)}{' '}
                            <Trans>MB</Trans>
                          </Badge>
                        )}
                        {job.execution_info?.output_size_bytes && (
                          <Badge variant="secondary" className="text-[10px]">
                            Out:{' '}
                            {(
                              job.execution_info.output_size_bytes /
                              1024 /
                              1024
                            ).toFixed(1)}{' '}
                            <Trans>MB</Trans>
                          </Badge>
                        )}
                      </div>
                    }
                    icon={<HardDrive className="h-4 w-4" />}
                  />
                </CardContent>
              </Card>
            </motion.div>

            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.4 }}
            >
              <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <Card className="bg-card/40 backdrop-blur-sm border-border/40 hover:bg-card/60 transition-colors">
                  <CardHeader className="pb-3">
                    <CardTitle className="text-base font-medium flex items-center gap-2">
                      <FileCode className="h-4 w-4 text-blue-500" />{' '}
                      <Trans>Input Path</Trans>
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="text-xs font-mono bg-muted/40 p-4 rounded-xl border border-border/40 text-muted-foreground leading-relaxed shadow-inner">
                      <div className="space-y-2.5 max-h-[160px] overflow-y-auto pr-2 custom-scrollbar">
                        {job.input_path.length > 0 ? (
                          job.input_path.map((path, i) => (
                            <div
                              key={i}
                              className="group/item flex items-start gap-3 p-2 rounded-lg hover:bg-background/80 hover:text-foreground transition-all duration-300 border border-transparent hover:border-border/50"
                            >
                              <FileCode className="h-3.5 w-3.5 mt-0.5 shrink-0 text-blue-500/50 group-hover/item:text-blue-500 transition-colors" />
                              <span className="flex-1 break-all tracking-tight selection:bg-blue-500/20">
                                {path}
                              </span>
                            </div>
                          ))
                        ) : (
                          <div className="flex flex-col items-center justify-center py-4 opacity-40">
                            <FileCode className="h-8 w-8 mb-2" />
                            <span className="italic">
                              <Trans>No input paths</Trans>
                            </span>
                          </div>
                        )}
                      </div>
                    </div>
                  </CardContent>
                </Card>

                <Card className="bg-card/40 backdrop-blur-sm border-border/40 hover:bg-card/60 transition-colors">
                  <CardHeader className="pb-3">
                    <CardTitle className="text-base font-medium flex items-center gap-2">
                      <FileOutput className="h-4 w-4 text-emerald-500" />{' '}
                      <Trans>Output Path</Trans>
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="text-xs font-mono bg-muted/40 p-4 rounded-xl border border-border/40 text-muted-foreground leading-relaxed shadow-inner">
                      <div className="space-y-2.5 max-h-[160px] overflow-y-auto pr-2 custom-scrollbar">
                        {job.output_path && job.output_path.length > 0 ? (
                          job.output_path.map((path, i) => (
                            <div
                              key={i}
                              className="group/item flex items-start gap-3 p-2 rounded-lg hover:bg-background/80 hover:text-foreground transition-all duration-300 border border-transparent hover:border-border/50"
                            >
                              <FileOutput className="h-3.5 w-3.5 mt-0.5 shrink-0 text-emerald-500/50 group-hover/item:text-emerald-500 transition-colors" />
                              <span className="flex-1 break-all tracking-tight selection:bg-emerald-500/20">
                                {path}
                              </span>
                            </div>
                          ))
                        ) : (
                          <div className="flex flex-col items-center justify-center py-4 opacity-40">
                            <FileOutput className="h-8 w-8 mb-2" />
                            <span className="italic">
                              <Trans>No output generated</Trans>
                            </span>
                          </div>
                        )}
                      </div>
                    </div>
                  </CardContent>
                </Card>
              </div>
            </motion.div>
          </div>

          {/* Right Column: Timing */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.35 }}
            className="h-full"
          >
            <Card className="h-full bg-card/40 backdrop-blur-sm border-border/40 shadow-sm flex flex-col">
              <CardHeader className="border-b border-border/40 pb-4">
                <CardTitle className="text-lg font-semibold flex items-center gap-2">
                  <Calendar className="h-5 w-5 text-primary/70" />
                  <Trans>Performance</Trans>
                </CardTitle>
              </CardHeader>
              <CardContent className="p-6 flex-1 flex flex-col gap-6">
                <div className="grid grid-cols-2 gap-4">
                  <div className="p-4 rounded-xl bg-background/50 border border-border/50 text-center">
                    <div className="text-muted-foreground text-xs font-medium uppercase tracking-wider mb-1">
                      <Trans>Duration</Trans>
                    </div>
                    <div className="text-2xl font-bold tracking-tight">
                      {formatDuration(job.duration_secs)}
                    </div>
                  </div>
                  <div className="p-4 rounded-xl bg-background/50 border border-border/50 text-center">
                    <div className="text-muted-foreground text-xs font-medium uppercase tracking-wider mb-1">
                      <Trans>Queue</Trans>
                    </div>
                    <div className="text-2xl font-bold tracking-tight text-muted-foreground">
                      {formatDuration(job.queue_wait_secs)}
                    </div>
                  </div>
                  {job.status === 'PROCESSING' && progressSnapshot && (
                    <>
                      <div className="p-4 rounded-xl bg-background/50 border border-border/50 text-center">
                        <div className="text-muted-foreground text-xs font-medium uppercase tracking-wider mb-1">
                          <Trans>Speed</Trans>
                        </div>
                        <div className="text-xl font-bold tracking-tight font-mono">
                          {progressSnapshot.speed_bytes_per_sec
                            ? `${(progressSnapshot.speed_bytes_per_sec / 1024 / 1024).toFixed(2)} ` +
                              i18n._(msg`MB/s`)
                            : '-'}
                        </div>
                      </div>
                      <div className="p-4 rounded-xl bg-background/50 border border-border/50 text-center">
                        <div className="text-muted-foreground text-xs font-medium uppercase tracking-wider mb-1">
                          <Trans>ETA</Trans>
                        </div>
                        <div className="text-xl font-bold tracking-tight font-mono">
                          {progressSnapshot.eta_secs
                            ? formatDuration(progressSnapshot.eta_secs)
                            : '-'}
                        </div>
                      </div>
                    </>
                  )}
                </div>
                <Separator className="bg-border/50" />
                <div className="space-y-6">
                  <TimelineItem
                    label={i18n._(msg`Created`)}
                    time={i18n.date(job.created_at, { timeStyle: 'medium' })}
                    active
                  />
                  <TimelineItem
                    label={i18n._(msg`Started`)}
                    time={
                      job.started_at
                        ? i18n.date(job.started_at, { timeStyle: 'medium' })
                        : undefined
                    }
                    active={!!job.started_at}
                  />
                  <TimelineItem
                    label={i18n._(msg`Completed`)}
                    time={
                      job.completed_at
                        ? i18n.date(job.completed_at, { timeStyle: 'medium' })
                        : undefined
                    }
                    active={!!job.completed_at}
                    isLast
                  />
                </div>
              </CardContent>
            </Card>
          </motion.div>

          {/* Full Width: Error & Logs */}
          <div className="lg:col-span-3 space-y-6">
            {job.error_message && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
              >
                <Card className="border-destructive/30 bg-destructive/5 backdrop-blur-sm shadow-sm overflow-hidden">
                  <div className="h-1 w-full bg-destructive/40" />
                  <CardHeader className="pb-2">
                    <CardTitle className="text-destructive flex items-center gap-2 text-base">
                      <AlertCircle className="h-5 w-5" />{' '}
                      <Trans>Error Details</Trans>
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="font-mono text-sm text-destructive/90 bg-background/50 p-4 rounded-lg border border-destructive/20 selection:bg-destructive/20">
                      {job.error_message}
                    </div>
                  </CardContent>
                </Card>
              </motion.div>
            )}

            {job.execution_info && (
              <motion.div
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: 0.5 }}
              >
                <Card className="border-border/40 bg-zinc-950 shadow-2xl overflow-hidden rounded-xl">
                  <div className="flex items-center justify-between px-4 py-3 bg-zinc-900/80 border-b border-zinc-800 backdrop-blur-md">
                    <div className="flex items-center gap-3">
                      <div className="flex gap-1.5">
                        <div className="w-3 h-3 rounded-full bg-red-500/20 border border-red-500/50" />
                        <div className="w-3 h-3 rounded-full bg-yellow-500/20 border border-yellow-500/50" />
                        <div className="w-3 h-3 rounded-full bg-green-500/20 border border-green-500/50" />
                      </div>
                      <div className="h-4 w-px bg-zinc-700 mx-1" />
                      <span className="text-zinc-400 text-xs font-mono font-medium flex items-center gap-2">
                        <Terminal className="h-3.5 w-3.5" /> execution-logs.log
                      </span>
                    </div>
                    <div className="flex items-center gap-2">
                      {(job.execution_info.log_error_count ?? 0) > 0 && (
                        <Badge
                          variant="outline"
                          className="text-[10px] border-red-500/30 text-red-400 bg-red-500/10 hover:bg-red-500/20 transition-colors"
                        >
                          {t(
                            i18n,
                          )`${plural(job.execution_info.log_error_count, { one: '# Error', other: '# Errors' })}`}
                        </Badge>
                      )}
                      {(job.execution_info.log_warn_count ?? 0) > 0 && (
                        <Badge
                          variant="outline"
                          className="text-[10px] border-yellow-500/30 text-yellow-400 bg-yellow-500/10 hover:bg-yellow-500/20 transition-colors"
                        >
                          {t(
                            i18n,
                          )`${plural(job.execution_info.log_warn_count, { one: '# Warning', other: '# Warnings' })}`}
                        </Badge>
                      )}
                      <Badge
                        variant="outline"
                        className="text-[10px] border-zinc-700 text-zinc-500 bg-zinc-900"
                      >
                        {job.execution_info.log_lines_total ? (
                          t(
                            i18n,
                          )`${plural(job.execution_info.log_lines_total, { one: '# Line', other: '# Lines' })}`
                        ) : (
                          <>
                            {i18n._(msg`Total`)} {totalLogs}
                          </>
                        )}
                      </Badge>
                    </div>
                  </div>

                  <div className="relative">
                    <ScrollArea className="h-[500px] w-full bg-zinc-950">
                      <div className="p-4 font-mono text-xs space-y-1">
                        {isLogsLoading && logs.length === 0 ? (
                          <div className="flex flex-col items-center justify-center h-[200px] text-zinc-700 space-y-2">
                            <RefreshCw className="h-6 w-6 animate-spin opacity-50" />
                            <p>
                              <Trans>Loading logs...</Trans>
                            </p>
                          </div>
                        ) : logs.length > 0 ? (
                          <>
                            {logs.map((log, i) => (
                              <div
                                key={i}
                                className="flex gap-4 hover:bg-zinc-900/50 -mx-2 px-2 py-0.5 rounded transition-colors group"
                              >
                                <span className="text-zinc-600 shrink-0 select-none w-20 group-hover:text-zinc-500 transition-colors">
                                  {i18n.date(log.timestamp, {
                                    timeStyle: 'medium',
                                  })}
                                </span>
                                <span
                                  className={cn(
                                    'break-all',
                                    log.level === 'ERROR'
                                      ? 'text-red-400 font-bold'
                                      : log.level === 'WARN'
                                        ? 'text-yellow-400'
                                        : 'text-zinc-300',
                                  )}
                                >
                                  {log.message}
                                </span>
                              </div>
                            ))}
                            {hasNextPage && (
                              <div
                                ref={loadMoreRef}
                                className="pt-2 flex justify-center py-4"
                              >
                                {isFetchingNextPage ? (
                                  <div className="flex items-center gap-2 text-muted-foreground text-xs">
                                    <RefreshCw className="h-3 w-3 animate-spin" />
                                    <Trans>Loading older logs...</Trans>
                                  </div>
                                ) : (
                                  <div className="h-8" />
                                )}
                              </div>
                            )}
                          </>
                        ) : (
                          <div className="flex flex-col items-center justify-center h-[200px] text-zinc-700">
                            <Terminal className="h-10 w-10 mb-3 opacity-20" />
                            <p>
                              <Trans>
                                No logs available for this execution.
                              </Trans>
                            </p>
                          </div>
                        )}
                        {/* Simulated Cursor */}
                        {!isFetchingNextPage && (
                          <div className="w-2.5 h-4 bg-zinc-500 animate-pulse mt-1 ml-20" />
                        )}
                      </div>
                    </ScrollArea>
                  </div>
                </Card>
              </motion.div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function InfoGroup({
  label,
  value,
  icon,
  mono,
}: {
  label: string;
  value: React.ReactNode;
  icon: React.ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground uppercase tracking-wider">
        {icon} {label}
      </div>
      <div
        className={cn(
          'text-sm font-medium text-foreground',
          mono && 'font-mono',
        )}
      >
        {value || <span className="opacity-50 italic">-</span>}
      </div>
    </div>
  );
}

function TimelineItem({
  label,
  time,
  active,
  isLast,
}: {
  label: string;
  time?: string | null;
  active: boolean;
  isLast?: boolean;
}) {
  return (
    <div className="relative pl-6">
      {!isLast && (
        <div
          className={cn(
            'absolute left-[7px] top-2 bottom-[-24px] w-px',
            active ? 'bg-primary/50' : 'bg-border',
          )}
        />
      )}
      <div
        className={cn(
          'absolute left-0 top-1.5 h-3.5 w-3.5 rounded-full border-2 transition-colors',
          active
            ? 'border-primary bg-primary ring-4 ring-primary/10'
            : 'border-muted-foreground/30 bg-background',
        )}
      />
      <div className="flex justify-between items-baseline">
        <span
          className={cn(
            'text-sm transition-colors',
            active ? 'font-medium text-foreground' : 'text-muted-foreground',
          )}
        >
          {label}
        </span>
        <span
          className={cn(
            'text-xs font-mono',
            active ? 'text-foreground' : 'text-muted-foreground/50',
          )}
        >
          {time || '-'}
        </span>
      </div>
      <div className="text-xs text-muted-foreground/50 mt-0.5 pl-0.5">
        {time || <span className="opacity-0">.</span>}
      </div>
    </div>
  );
}
