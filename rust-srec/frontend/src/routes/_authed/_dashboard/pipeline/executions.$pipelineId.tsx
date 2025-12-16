import { Link, createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  listPipelineJobsPage,
  retryPipelineJob,
  cancelPipeline,
} from '@/server/functions';
import { Card, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  ArrowLeft,
  Clock,
  RefreshCw,
  CheckCircle2,
  XCircle,
  AlertCircle,
  Calendar,
  ArrowRight,
  StopCircle,
  Timer,
  Layers,
} from 'lucide-react';
import { Trans, useLingui } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { toast } from 'sonner';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/executions/$pipelineId',
)({
  component: PipelineExecutionPage,
});

import { formatDuration } from '@/lib/format';

const STATUS_CONFIG: Record<
  string,
  {
    icon: any;
    color: string;
    badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline';
    animate?: boolean;
    gradient: string;
  }
> = {
  PENDING: {
    icon: Clock,
    color: 'text-muted-foreground',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
  },
  PROCESSING: {
    icon: RefreshCw,
    color: 'text-blue-500',
    badgeVariant: 'default',
    animate: true,
    gradient: 'from-blue-500/20 to-blue-500/5',
  },
  COMPLETED: {
    icon: CheckCircle2,
    color: 'text-emerald-500',
    badgeVariant: 'secondary',
    gradient: 'from-emerald-500/20 to-emerald-500/5',
  },
  FAILED: {
    icon: XCircle,
    color: 'text-red-500',
    badgeVariant: 'destructive',
    gradient: 'from-red-500/20 to-red-500/5',
  },
  CANCELLED: {
    icon: AlertCircle,
    color: 'text-gray-500',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
  },
  INTERRUPTED: {
    icon: AlertCircle,
    color: 'text-orange-500',
    badgeVariant: 'secondary',
    gradient: 'from-orange-500/20 to-orange-500/5',
  },
};

function PipelineExecutionPage() {
  const { pipelineId } = Route.useParams();
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  const { data, isLoading, error } = useQuery({
    queryKey: ['pipeline', 'executions', pipelineId],
    queryFn: () => listPipelineJobsPage({ data: { pipeline_id: pipelineId } }),
    refetchInterval: (query) => {
      const jobs = query.state.data?.items || [];
      const isActive = jobs.some((j) =>
        ['PENDING', 'PROCESSING'].includes(j.status),
      );
      return isActive ? 1000 : false;
    },
  });

  const retryMutation = useMutation({
    mutationFn: (id: string) => retryPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(t`Job retry initiated`);
      queryClient.invalidateQueries({
        queryKey: ['pipeline', 'executions', pipelineId],
      });
    },
    onError: () => toast.error(t`Failed to retry job`),
  });

  const cancelMutation = useMutation({
    mutationFn: (pipelineId: string) => cancelPipeline({ data: pipelineId }),
    onSuccess: (result) => {
      toast.success(t`Cancelled ${result.cancelled_count} jobs in pipeline`);
      queryClient.invalidateQueries({
        queryKey: ['pipeline', 'executions', pipelineId],
      });
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'stats'] });
    },
    onError: () => toast.error(t`Failed to cancel pipeline`),
  });

  if (isLoading) {
    return (
      <div className="min-h-screen bg-background p-6 space-y-8">
        <div className="max-w-7xl mx-auto space-y-8">
          <div className="flex items-center gap-6">
            <Skeleton className="h-16 w-16 rounded-2xl" />
            <div className="space-y-3">
              <Skeleton className="h-8 w-64" />
              <Skeleton className="h-4 w-32" />
            </div>
          </div>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center p-6">
        <Alert
          variant="destructive"
          className="max-w-lg shadow-2xl bg-background/95 backdrop-blur-xl border-destructive/20"
        >
          <AlertCircle className="h-4 w-4" />
          <AlertTitle>
            <Trans>Error Loading Pipeline</Trans>
          </AlertTitle>
          <AlertDescription>{error.message}</AlertDescription>
          <Button
            variant="outline"
            className="mt-4 w-full"
            onClick={() => window.history.back()}
          >
            <ArrowLeft className="mr-2 h-4 w-4" />{' '}
            <Trans>Return Previous Page</Trans>
          </Button>
        </Alert>
      </div>
    );
  }

  const jobs = data?.items || [];
  const sortedJobs = [...jobs].sort(
    (a, b) =>
      new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
  );

  if (sortedJobs.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center min-h-[60vh] text-center space-y-4">
        <div className="p-4 rounded-full bg-muted/30 mb-4">
          <Layers className="h-12 w-12 text-muted-foreground/50" />
        </div>
        <h3 className="text-xl font-semibold text-foreground/80">
          <Trans>No Jobs Found</Trans>
        </h3>
        <p className="text-muted-foreground">
          <Trans>This pipeline sequence appears to be empty.</Trans>
        </p>
        <Button variant="outline" onClick={() => window.history.back()}>
          <ArrowLeft className="mr-2 h-4 w-4" /> <Trans>Go Back</Trans>
        </Button>
      </div>
    );
  }

  const firstJob = sortedJobs[0];
  const isFailed = sortedJobs.some((j) => j.status === 'FAILED');
  const isProcessing = sortedJobs.some((j) => j.status === 'PROCESSING');
  const isPending = sortedJobs.some((j) => j.status === 'PENDING');
  const isCompleted = sortedJobs.every((j) => j.status === 'COMPLETED');

  const overallStatus = isFailed
    ? 'FAILED'
    : isProcessing
      ? 'PROCESSING'
      : isCompleted
        ? 'COMPLETED'
        : isPending
          ? 'PENDING'
          : 'PROCESSING';
  const statusConfig = STATUS_CONFIG[overallStatus] || STATUS_CONFIG.PENDING;
  const StatusIcon = statusConfig.icon;

  const totalDuration = sortedJobs.reduce(
    (acc, job) => acc + (job.duration_secs || 0),
    0,
  );

  return (
    <div className="relative min-h-screen overflow-hidden selection:bg-primary/20">
      {/* Background Decoration */}
      <div className="fixed inset-0 pointer-events-none">
        <div className="absolute top-0 right-0 -mt-20 -mr-20 w-[500px] h-[500px] bg-primary/5 rounded-full blur-[120px]" />
        <div className="absolute bottom-0 left-0 -mb-40 -ml-20 w-[600px] h-[600px] bg-blue-500/5 rounded-full blur-[120px]" />
      </div>

      <div className="relative z-10 max-w-7xl mx-auto px-6 py-8 pb-32">
        {/* Navigation & Header */}
        <div className="flex flex-col gap-8 mb-12">
          <motion.div
            initial={{ opacity: 0, x: -20 }}
            animate={{ opacity: 1, x: 0 }}
            className="flex items-center gap-2"
          >
            <Button
              variant="ghost"
              size="sm"
              asChild
              className="group text-muted-foreground hover:text-foreground hover:bg-transparent px-0"
            >
              <Link to="/pipeline/jobs" className="flex items-center">
                <ArrowLeft className="mr-2 h-4 w-4 transition-transform group-hover:-translate-x-1" />
                <Trans>Back to Pipeline Jobs</Trans>
              </Link>
            </Button>
          </motion.div>

          <div className="flex flex-col md:flex-row md:items-start justify-between gap-6">
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.1 }}
              className="space-y-4"
            >
              <div className="flex items-center gap-4">
                <div
                  className={cn(
                    'flex items-center justify-center w-16 h-16 rounded-2xl shadow-xl ring-1 ring-white/10 backdrop-blur-md bg-gradient-to-br',
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
                  <div className="flex items-center gap-3 mb-1">
                    <h1 className="text-3xl font-bold tracking-tight">
                      <Trans>Pipeline Execution</Trans>
                    </h1>
                    <Badge
                      variant="outline"
                      className={cn(
                        'text-xs font-mono uppercase tracking-wider bg-background/50 backdrop-blur border-border/50',
                        statusConfig.color,
                      )}
                    >
                      {overallStatus}
                    </Badge>
                  </div>
                  <p className="text-muted-foreground font-mono text-sm opacity-80">
                    ID: {pipelineId}
                  </p>
                </div>
              </div>
            </motion.div>

            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.2 }}
              className="flex items-center gap-3"
            >
              {(isPending || isProcessing) && (
                <Button
                  variant="destructive"
                  className="shadow-lg shadow-destructive/20 hover:shadow-destructive/40 transition-shadow"
                  onClick={() => cancelMutation.mutate(pipelineId)}
                  disabled={cancelMutation.isPending}
                >
                  <StopCircle className="mr-2 h-4 w-4" />{' '}
                  <Trans>Cancel Pipeline</Trans>
                </Button>
              )}
              {isFailed && (
                <Button
                  className="bg-primary shadow-lg shadow-primary/20 hover:shadow-primary/40 transition-shadow"
                  onClick={() =>
                    sortedJobs
                      .filter((j) => j.status === 'FAILED')
                      .forEach((j) => retryMutation.mutate(j.id))
                  }
                  disabled={retryMutation.isPending}
                >
                  <RefreshCw
                    className={cn(
                      'mr-2 h-4 w-4',
                      retryMutation.isPending && 'animate-spin',
                    )}
                  />
                  <Trans>Retry Failed Steps</Trans>
                </Button>
              )}
            </motion.div>
          </div>
        </div>

        {/* KPI Grid */}
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.3 }}
          className="grid grid-cols-2 lg:grid-cols-4 gap-4 mb-16"
        >
          <StatsCard
            icon={<Timer className="h-5 w-5 text-blue-400" />}
            label={t`Total Duration`}
            value={formatDuration(totalDuration)}
            delay={0.1}
          />
          <StatsCard
            icon={<Layers className="h-5 w-5 text-purple-400" />}
            label={t`Total Steps`}
            value={sortedJobs.length}
            delay={0.2}
          />
          <StatsCard
            icon={<CheckCircle2 className="h-5 w-5 text-emerald-400" />}
            label={t`Completed`}
            value={sortedJobs.filter((j) => j.status === 'COMPLETED').length}
            delay={0.3}
          />
          <StatsCard
            icon={<Calendar className="h-5 w-5 text-orange-400" />}
            label={t`Started`}
            value={i18n.date(firstJob.created_at, { timeStyle: 'short' })}
            subtext={i18n.date(firstJob.created_at, { dateStyle: 'medium' })}
            delay={0.4}
          />
        </motion.div>

        {/* Visual Timeline */}
        <div className="relative">
          <div className="absolute left-[27px] top-4 bottom-8 w-px bg-gradient-to-b from-border via-border/50 to-transparent md:left-1/2 md:-ml-px" />

          <div className="space-y-12">
            {sortedJobs.map((job, index) => {
              const jobConfig =
                STATUS_CONFIG[job.status] || STATUS_CONFIG.PENDING;
              const JobIcon = jobConfig.icon;
              const isEven = index % 2 === 0;

              return (
                <motion.div
                  key={job.id}
                  initial={{ opacity: 0, y: 50 }}
                  whileInView={{ opacity: 1, y: 0 }}
                  viewport={{ once: true, margin: '-50px' }}
                  transition={{ duration: 0.5, delay: index * 0.1 }}
                  className={cn(
                    'relative flex items-center gap-8',
                    'md:justify-center',
                  )}
                >
                  {/* Timeline Node */}
                  <div
                    className={cn(
                      'absolute left-0 w-14 h-14 rounded-full border-4 border-background flex items-center justify-center shrink-0 z-10 transition-transform duration-300 hover:scale-110 shadow-lg',
                      'md:left-1/2 md:-ml-7',
                      jobConfig.color,
                      'bg-card',
                    )}
                  >
                    <div
                      className={cn(
                        'absolute inset-0 rounded-full opacity-20',
                        jobConfig.gradient,
                      )}
                    />
                    <JobIcon
                      className={cn(
                        'h-6 w-6 relative z-10',
                        jobConfig.animate && 'animate-spin',
                      )}
                    />
                  </div>

                  {/* Content Card */}
                  <div
                    className={cn(
                      'flex-1 ml-20 md:ml-0 md:w-1/2',
                      isEven ? 'md:pr-20 md:text-right' : 'md:pl-20 md:order-1',
                    )}
                  >
                    <Link
                      to="/pipeline/jobs/$jobId"
                      params={{ jobId: job.id }}
                      className="block group"
                    >
                      <Card className="overflow-hidden border-border/40 bg-card/40 backdrop-blur-sm transition-all duration-300 hover:shadow-2xl hover:-translate-y-1 hover:border-primary/20 hover:bg-card/60">
                        {job.status === 'PROCESSING' && (
                          <div className="h-1 w-full bg-muted/50">
                            <div className="h-full bg-blue-500 animate-[progress_1s_ease-in-out_infinite]" />
                          </div>
                        )}
                        <CardContent className="p-6">
                          <div
                            className={cn(
                              'flex flex-col gap-1 mb-4',
                              isEven ? 'md:items-end' : '',
                            )}
                          >
                            <Badge
                              variant="outline"
                              className="w-fit mb-2 font-mono text-xs uppercase opacity-70"
                            >
                              {job.processor_type}
                            </Badge>
                            <h3 className="text-lg font-semibold tracking-tight group-hover:text-primary transition-colors">
                              {job.processor_type.replace(/_/g, ' ')}
                            </h3>
                            <p className="text-xs text-muted-foreground font-mono">
                              {job.id.split('-')[0]}...{job.id.slice(-8)}
                            </p>
                          </div>

                          <div
                            className={cn(
                              'grid grid-cols-2 gap-4 text-sm text-muted-foreground',
                              isEven ? 'md:text-right' : '',
                            )}
                          >
                            <div>
                              <span className="block text-xs uppercase tracking-wider opacity-60">
                                <Trans>Duration</Trans>
                              </span>
                              <span className="font-medium text-foreground">
                                {formatDuration(job.duration_secs)}
                              </span>
                            </div>
                            <div>
                              <span className="block text-xs uppercase tracking-wider opacity-60">
                                <Trans>Finished</Trans>
                              </span>
                              <span className="font-medium text-foreground">
                                {job.completed_at
                                  ? i18n.date(job.completed_at, {
                                      timeStyle: 'medium',
                                    })
                                  : '-'}
                              </span>
                            </div>
                          </div>

                          {job.error_message && (
                            <div className="mt-4 p-3 rounded-lg bg-destructive/10 border border-destructive/20 text-destructive text-xs font-medium">
                              <div className="flex items-center gap-2 mb-1">
                                <AlertCircle className="h-3 w-3" />
                                <span className="uppercase tracking-wider">
                                  <Trans>Error Output</Trans>
                                </span>
                              </div>
                              <p className="line-clamp-2 opacity-90">
                                {job.error_message}
                              </p>
                            </div>
                          )}
                        </CardContent>
                        <div className="px-6 py-2 bg-muted/20 border-t border-border/20 flex items-center justify-between text-xs text-muted-foreground group-hover:bg-primary/5 transition-colors">
                          <span className="font-medium">
                            <Trans>View Output & Logs</Trans>
                          </span>
                          <ArrowRight className="h-3 w-3 transition-transform group-hover:translate-x-1" />
                        </div>
                      </Card>
                    </Link>
                  </div>

                  {/* Empty spacer for the other side on desktop */}
                  <div className="hidden md:block md:w-1/2" />
                </motion.div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

function StatsCard({
  icon,
  label,
  value,
  subtext,
  delay,
}: {
  icon: any;
  label: string;
  value: any;
  subtext?: string;
  delay: number;
}) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ delay, duration: 0.4 }}
      className="h-full"
    >
      <Card className="bg-card/30 backdrop-blur border-border/40 hover:bg-card/50 transition-colors h-full flex flex-col justify-center">
        <CardContent className="p-6 flex items-start justify-between">
          <div>
            <p className="text-sm font-medium text-muted-foreground mb-1">
              {label}
            </p>
            <h4 className="text-2xl font-bold tracking-tight text-foreground">
              {value}
            </h4>
            {subtext && (
              <p className="text-xs text-muted-foreground mt-1">{subtext}</p>
            )}
          </div>
          <div className="p-3 rounded-xl bg-background/50 ring-1 ring-border/50">
            {icon}
          </div>
        </CardContent>
      </Card>
    </motion.div>
  );
}
