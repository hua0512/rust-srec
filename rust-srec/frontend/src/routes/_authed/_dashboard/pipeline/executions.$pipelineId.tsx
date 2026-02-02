import { Link, createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  getDagExecution,
  getDagGraph,
  retryDagSteps,
  cancelDag,
} from '@/server/functions';
import { DagGraphView } from '@/components/pipeline/dag-graph-view';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { Card, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  ArrowLeft,
  RefreshCw,
  CheckCircle2,
  XCircle,
  AlertCircle,
  Calendar,
  ArrowRight,
  StopCircle,
  Timer,
  Layers,
  ExternalLink,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg, t, plural } from '@lingui/core/macro';
import { toast } from 'sonner';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';
import { getJobPresetName } from '@/components/pipeline/presets/default-presets-i18n';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { type DagStep } from '@/api/schemas';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/executions/$pipelineId',
)({
  component: PipelineExecutionPage,
});

import { STATUS_CONFIG } from '@/components/pipeline/status-config';

function PipelineExecutionPage() {
  const { pipelineId } = Route.useParams();
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  const {
    data: dag,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['pipeline', 'executions', pipelineId, 'status'],
    queryFn: () => getDagExecution({ data: pipelineId }),
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return ['PENDING', 'PROCESSING'].includes(status || '') ? 1000 : false;
    },
  });

  const { data: graph } = useQuery({
    queryKey: ['pipeline', 'executions', pipelineId, 'graph'],
    queryFn: () => getDagGraph({ data: pipelineId }),
    enabled: !!dag,
    refetchInterval: () => {
      return ['PENDING', 'PROCESSING'].includes(dag?.status || '')
        ? 2000
        : false;
    },
  });

  const retryMutation = useMutation({
    mutationFn: (id: string) => retryDagSteps({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Failed steps retry initiated`));
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'executions', pipelineId],
      });
    },
    onError: () => toast.error(i18n._(msg`Failed to retry steps`)),
  });

  const cancelMutation = useMutation({
    mutationFn: (pipelineId: string) => cancelDag({ data: pipelineId }),
    onSuccess: (result) => {
      toast.success(
        i18n._(msg`Cancelled ${result.cancelled_steps} steps in pipeline`),
      );
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'executions', pipelineId],
      });
      void queryClient.invalidateQueries({ queryKey: ['pipeline', 'stats'] });
    },
    onError: () => toast.error(i18n._(msg`Failed to cancel pipeline`)),
  });

  if (isLoading || !dag) {
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

  const steps = dag.steps || [];

  const overallStatus = dag?.status || 'PENDING';
  const statusConfig = STATUS_CONFIG[overallStatus] || STATUS_CONFIG.PENDING;
  const StatusIcon = statusConfig.icon;

  return (
    <div className="relative min-h-screen overflow-hidden selection:bg-primary/20">
      {/* Background Decoration */}
      <div className="fixed inset-0 pointer-events-none overflow-hidden">
        <div className="absolute top-0 right-0 -mt-20 -mr-20 w-[500px] h-[500px] bg-primary/5 rounded-full blur-[120px] animate-pulse" />
        <div className="absolute top-1/2 left-0 -translate-y-1/2 -ml-20 w-[400px] h-[400px] bg-purple-500/5 rounded-full blur-[100px]" />
        <div className="absolute bottom-0 right-1/4 -mb-40 w-[600px] h-[600px] bg-blue-500/5 rounded-full blur-[120px]" />
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_120%,rgba(120,119,198,0.05),rgba(255,255,255,0))]" />
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
                      statusConfig.textColor,
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
                        'text-xs font-mono uppercase tracking-wider bg-background/50 backdrop-blur border-border/50 relative overflow-hidden',
                        statusConfig.textColor,
                      )}
                    >
                      {overallStatus === 'PROCESSING' && (
                        <span className="absolute inset-0 bg-current opacity-10 animate-pulse pointer-events-none" />
                      )}
                      {i18n._(
                        overallStatus === 'PENDING'
                          ? msg`Pending`
                          : overallStatus === 'PROCESSING'
                            ? msg`Processing`
                            : overallStatus === 'COMPLETED'
                              ? msg`Completed`
                              : overallStatus === 'FAILED'
                                ? msg`Failed`
                                : overallStatus === 'INTERRUPTED'
                                  ? msg`Interrupted`
                                  : overallStatus,
                      )}
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
              {(overallStatus === 'PENDING' ||
                overallStatus === 'PROCESSING') && (
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
              {overallStatus === 'FAILED' && (
                <Button
                  className="bg-primary shadow-lg shadow-primary/20 hover:shadow-primary/40 transition-shadow"
                  onClick={() => retryMutation.mutate(pipelineId)}
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
            label={i18n._(msg`Progress`)}
            value={`${(dag.progress_percent || 0).toFixed(1)}%`}
            delay={0.1}
          />
          <StatsCard
            icon={<Layers className="h-5 w-5 text-purple-400" />}
            label={i18n._(msg`Total Steps`)}
            value={dag.total_steps}
            delay={0.2}
          />
          <StatsCard
            icon={<CheckCircle2 className="h-5 w-5 text-emerald-400" />}
            label={i18n._(msg`Completed`)}
            value={dag.completed_steps}
            delay={0.3}
          />
          <StatsCard
            icon={<Calendar className="h-5 w-5 text-orange-400" />}
            label={i18n._(msg`Started`)}
            value={i18n.date(dag.created_at, { timeStyle: 'short' })}
            subtext={i18n.date(dag.created_at, { dateStyle: 'medium' })}
            delay={0.4}
          />
        </motion.div>

        <Tabs defaultValue="graph" className="w-full">
          <TabsList className="bg-muted/30 backdrop-blur-sm border border-border/40 p-1 h-11 rounded-full gap-1 mb-12 max-w-md mx-auto">
            <TabsTrigger
              value="graph"
              className="rounded-full px-6 transition-all data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-md hover:text-foreground/80"
            >
              <Trans>DAG Graph</Trans>
            </TabsTrigger>
            <TabsTrigger
              value="list"
              className="rounded-full px-6 transition-all data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-md hover:text-foreground/80"
            >
              <Trans>Steps List</Trans>
            </TabsTrigger>
          </TabsList>

          <TabsContent value="graph" className="mt-0">
            {graph ? (
              <DagGraphView graph={graph} />
            ) : (
              <div className="h-[500px] flex items-center justify-center border border-dashed rounded-xl">
                <Skeleton className="h-[400px] w-full max-w-3xl rounded-xl" />
              </div>
            )}
          </TabsContent>

          <TabsContent value="list" className="mt-0">
            <div className="space-y-4">
              {steps.map((step) => {
                const jobConfig =
                  STATUS_CONFIG[step.status] || STATUS_CONFIG.PENDING;
                return (
                  <div key={step.step_id} className="block">
                    {step.job_id ? (
                      <Link
                        to="/pipeline/jobs/$jobId"
                        params={{ jobId: step.job_id }}
                        className="group"
                      >
                        <StepCard step={step} jobConfig={jobConfig} />
                      </Link>
                    ) : (
                      <StepCard step={step} jobConfig={jobConfig} />
                    )}
                  </div>
                );
              })}
            </div>
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}

function StepCard({
  step,
  jobConfig,
}: {
  step: DagStep;
  jobConfig: (typeof STATUS_CONFIG)[keyof typeof STATUS_CONFIG];
}) {
  const { i18n } = useLingui();
  const isProcessing = step.status === 'PROCESSING';
  const isCompleted = step.status === 'COMPLETED';
  const isFailed = step.status === 'FAILED';

  const StatusIcon = jobConfig.icon;

  return (
    <Card className="group relative overflow-hidden border-border/40 bg-card/40 backdrop-blur-md transition-all duration-500 hover:shadow-2xl hover:-translate-y-1 hover:border-primary/30 hover:bg-card/60">
      {/* Top Progress Bar for active jobs */}
      {isProcessing && (
        <div className="absolute top-0 left-0 right-0 h-1 bg-blue-500/10 overflow-hidden">
          <motion.div
            className="h-full bg-blue-500 shadow-[0_0_10px_rgba(59,130,246,0.5)]"
            initial={{ x: '-100%' }}
            animate={{ x: '100%' }}
            transition={{ repeat: Infinity, duration: 1.5, ease: 'linear' }}
          />
        </div>
      )}

      {/* Background Pattern */}
      <div className="absolute top-0 right-0 p-4 opacity-[0.03] group-hover:opacity-[0.05] transition-opacity pointer-events-none">
        <Layers className="h-24 w-24 rotate-12" />
      </div>

      <CardContent className="p-6">
        <div className="flex flex-col md:flex-row md:items-center justify-between gap-6">
          <div className="space-y-3 flex-1">
            <div className="flex items-center gap-3">
              <div
                className={cn(
                  'p-2 rounded-lg ring-1 ring-inset transition-colors duration-500',
                  jobConfig.gradient,
                  'ring-white/5',
                )}
              >
                <StatusIcon
                  className={cn(
                    'h-4 w-4',
                    jobConfig.textColor,
                    isProcessing && 'animate-spin',
                  )}
                />
              </div>
              <Badge
                variant="outline"
                className="font-mono text-[10px] uppercase opacity-60 tracking-tight"
              >
                {(() => {
                  const def = getProcessorDefinition(step.processor);
                  return def ? i18n._(def.label) : step.processor;
                })()}
              </Badge>
            </div>

            <div>
              <h3 className="text-xl font-bold tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                {getJobPresetName(
                  { id: step.step_id, name: step.step_id },
                  i18n,
                ).replace(/_/g, ' ')}
              </h3>
              {step.job_id && (
                <div className="flex items-center gap-1.5 mt-1 text-xs text-muted-foreground/60 font-mono">
                  <div className="w-1.5 h-1.5 rounded-full bg-current opacity-40" />
                  ID: {step.job_id.substring(0, 8)}...{step.job_id.slice(-4)}
                </div>
              )}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-8 md:text-right">
            <div className="space-y-1">
              <span className="block text-[10px] uppercase tracking-widest font-bold text-muted-foreground/40">
                <Trans>Status</Trans>
              </span>
              <div className="flex items-center md:justify-end gap-2">
                <span
                  className={cn(
                    'text-sm font-semibold tracking-wide',
                    jobConfig.textColor,
                  )}
                >
                  {i18n._(
                    step.status === 'PENDING'
                      ? msg`Pending`
                      : step.status === 'PROCESSING'
                        ? msg`Processing`
                        : step.status === 'COMPLETED'
                          ? msg`Completed`
                          : step.status === 'FAILED'
                            ? msg`Failed`
                            : step.status === 'CANCELLED'
                              ? msg`Cancelled`
                              : ({ id: step.status } as any),
                  )}
                </span>
                {isCompleted && (
                  <CheckCircle2 className="h-4 w-4 text-emerald-500" />
                )}
                {isFailed && <XCircle className="h-4 w-4 text-red-500" />}
              </div>
            </div>

            <div className="space-y-1">
              <span className="block text-[10px] uppercase tracking-widest font-bold text-muted-foreground/40">
                <Trans>Outputs</Trans>
              </span>
              <div className="flex items-center md:justify-end gap-2 text-foreground/80">
                <span className="text-sm font-semibold">
                  {step.outputs.length}
                </span>
                <span className="text-xs opacity-60 font-medium">
                  {t(
                    i18n,
                  )`${plural(step.outputs.length, { one: 'file', other: 'files' })}`}
                </span>
              </div>
            </div>
          </div>
        </div>
      </CardContent>

      {step.job_id && (
        <div className="px-6 py-3 bg-muted/10 border-t border-border/10 flex items-center justify-between group-hover:bg-primary/5 transition-colors duration-300">
          <div className="flex items-center gap-2 text-[11px] font-medium text-muted-foreground/80 group-hover:text-primary/80 transition-colors">
            <ExternalLink className="h-3.5 w-3.5" />
            <Trans>View Detailed Logs & Outputs</Trans>
          </div>
          <motion.div
            initial={{ x: 0 }}
            whileHover={{ x: 5 }}
            transition={{ type: 'spring', stiffness: 400, damping: 10 }}
          >
            <ArrowRight className="h-4 w-4 text-primary/40 group-hover:text-primary transition-colors" />
          </motion.div>
        </div>
      )}
    </Card>
  );
}

function StatsCard({
  icon,
  label,
  value,
  subtext,
  delay,
}: {
  icon: React.ReactNode;
  label: string;
  value: string | number;
  subtext?: string;
  delay: number;
}) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{
        delay,
        duration: 0.5,
        type: 'spring',
        stiffness: 100,
        damping: 15,
      }}
      className="h-full group"
    >
      <Card className="relative overflow-hidden bg-card/30 backdrop-blur-xl border-border/40 hover:bg-card/50 hover:border-primary/20 transition-all duration-500 h-full flex flex-col justify-center">
        {/* Subtle hover glow */}
        <div className="absolute inset-0 bg-primary/5 opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-700 pointer-events-none" />

        <CardContent className="p-6 flex items-start justify-between relative z-10">
          <div className="space-y-1">
            <p className="text-[10px] font-bold uppercase tracking-[0.2em] text-muted-foreground/60 mb-2">
              {label}
            </p>
            <h4 className="text-3xl font-extrabold tracking-tight text-foreground decoration-primary/20 decoration-2 transition-colors group-hover:text-primary/90">
              {value}
            </h4>
            {subtext && (
              <div className="flex items-center gap-1.5 mt-2">
                <div className="w-1 h-1 rounded-full bg-primary/40" />
                <p className="text-[10px] font-medium text-muted-foreground/80 tracking-wide uppercase">
                  {subtext}
                </p>
              </div>
            )}
          </div>
          <div className="p-3.5 rounded-2xl bg-gradient-to-br from-background/80 to-background/20 ring-1 ring-white/10 shadow-xl group-hover:scale-110 group-hover:rotate-3 transition-transform duration-500 bg-background/50">
            {icon}
          </div>
        </CardContent>
      </Card>
    </motion.div>
  );
}
