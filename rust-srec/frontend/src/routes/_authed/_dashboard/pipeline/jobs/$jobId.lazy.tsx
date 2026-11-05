import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Link, createLazyFileRoute } from '@tanstack/react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  getPipelineJob,
  getPipelineJobProgress,
  getPipelineJobUploads,
} from '@/server/functions/pipeline';
import type { Job } from '@/api/schemas/pipeline';
import type { JobProgressSnapshot } from '@/api/schemas/session';
import { hasReportedProgress, liveProgressSnapshot } from '@/lib/job-progress';
import { useWebSocket } from '@/providers/WebSocketContext';
import { useUploadStore } from '@/store/uploads';
import { useJobActions } from '@/components/pipeline/jobs/use-job-actions';
import { useJobLogs } from '@/components/pipeline/jobs/use-job-logs';
import { JobProgressPanel } from '@/components/pipeline/jobs/job-progress-panel';
import { JobInterruptedOutputsCard } from '@/components/pipeline/jobs/job-interrupted-outputs-card';
import { JobUploadsCard } from '@/components/pipeline/jobs/job-uploads-card';
import { JobLogViewer } from '@/components/pipeline/jobs/job-log-viewer';
import { StalledOperationBanner } from '@/components/pipeline/jobs/stalled-operation-banner';
import { RetryCountdown } from '@/components/pipeline/retry-countdown';
import { ConfirmDialog } from '@/components/shared/confirm-dialog';
import {
  AmbientBackground,
  BackLink,
  DetailLoadError,
} from '@/components/shared/detail-page';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import { Separator } from '@/components/ui/separator';

import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';
import { formatBytes } from '@/lib/format';
import {
  getStatusConfig,
  getStatusLabel,
} from '@/components/pipeline/status-config';
import {
  AlertCircle,
  RotateCcw,
  Workflow,
  Box,
  Cpu,
  Calendar,
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

import { formatDate } from '@/lib/datetime';

/**
 * How often a queued or running job is re-read. Its progress arrives over the
 * live-updates socket, and the socket's end-of-job events trigger an
 * immediate read, so this only has to catch status changes.
 */
const ACTIVE_JOB_POLL_MS = 3_000;

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
      const data = query.state.data;
      if (data?.status === 'PROCESSING' || data?.status === 'PENDING') {
        return ACTIVE_JOB_POLL_MS;
      }
      // The retry sweeper re-queues a due retry within about 15 s, so a slower
      // poll still follows the job back to PENDING.
      return data?.status === 'FAILED' && data.retry_after ? 5000 : false;
    },
  });

  // Every interval below runs only while the job is queued or running.
  const jobStatus = job?.status;
  const isJobActive = jobStatus === 'PROCESSING' || jobStatus === 'PENDING';
  const isJobTerminal =
    jobStatus === 'COMPLETED' ||
    jobStatus === 'FAILED' ||
    jobStatus === 'CANCELLED';

  // Uploads and jobs that have run for a while report their progress over
  // the live-updates socket; the endpoint is polled only while that has
  // nothing for this job. Only these booleans are read here: the figures
  // change twice a second and are read by LiveJobProgressPanel alone, so the
  // page and its logs do not re-render with them.
  const { isConnected } = useWebSocket();
  const hasLiveView = useUploadStore(
    (state) => state.uploadsByJobId.has(jobId) || state.jobsByJobId.has(jobId),
  );
  const hasLiveProgress = useUploadStore((state) =>
    hasReportedProgress(
      state.uploadsByJobId.get(jobId) ?? state.jobsByJobId.get(jobId),
    ),
  );
  const showLiveProgress =
    isConnected && jobStatus === 'PROCESSING' && hasLiveProgress;

  // The socket announces the end of a job it was reporting before the next
  // poll would notice; read the job straight away.
  const hadLiveViewRef = useRef(false);
  useEffect(() => {
    if (hadLiveViewRef.current && !hasLiveView && isConnected) {
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'job', jobId],
        exact: true,
      });
    }
    hadLiveViewRef.current = hasLiveView;
  }, [hasLiveView, isConnected, jobId, queryClient]);

  const { data: polledSnapshot } = useQuery({
    queryKey: ['pipeline', 'job', jobId, 'progress'],
    queryFn: () => getPipelineJobProgress({ data: { id: jobId } }),
    enabled: jobStatus === 'PROCESSING' && !showLiveProgress,
    refetchInterval: 1000,
    retry: false, // Don't retry on 404 when no progress is available
    throwOnError: false, // Silently handle errors - progress is optional
  });

  // Durable per-file upload results. Records land when the job reaches a
  // terminal state, so refetch while active and settle once terminal.
  const { data: uploadsData } = useQuery({
    queryKey: ['pipeline', 'job', jobId, 'uploads'],
    queryFn: () => getPipelineJobUploads({ data: { id: jobId } }),
    refetchInterval: isJobActive ? 5000 : false,
    enabled: !!job,
    throwOnError: false, // The card simply doesn't render without records
  });
  const uploadRecords = uploadsData?.items ?? [];

  // The backend writes upload_records after flipping the job terminal, and
  // turning refetchInterval off does not trigger a final fetch — so the last
  // interval fetch predates the records. Refetch once on the terminal
  // transition so the Uploads card appears without a remount.
  useEffect(() => {
    if (!isJobTerminal) return;
    void queryClient.invalidateQueries({
      queryKey: ['pipeline', 'job', jobId, 'uploads'],
    });
  }, [isJobTerminal, jobId, queryClient]);

  const {
    data: logsData,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
    isLoading: isLogsLoading,
  } = useJobLogs({ jobId, enabled: !!job, isJobActive, isJobTerminal });

  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
  const {
    retryMutation,
    retryWorkflowMutation,
    cancelMutation,
    deleteMutation,
  } = useJobActions(jobId);

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
        <div className="@container">
          <div className="grid grid-cols-1 @3xl:grid-cols-3 gap-6">
            <Skeleton className="h-[200px] @3xl:col-span-2 rounded-2xl" />
            <Skeleton className="h-[200px] rounded-2xl" />
          </div>
        </div>
        <Skeleton className="h-[400px] rounded-2xl" />
      </div>
    );
  }

  if (isError || !job) {
    return (
      <DetailLoadError
        title={<Trans>Error Loading Job</Trans>}
        error={error}
        notFound={<Trans>Job not found</Trans>}
        back={{ to: '/pipeline/jobs', label: <Trans>Back to Jobs</Trans> }}
      />
    );
  }

  // A failed workflow step waiting for its automatic retry is not finished.
  const retryScheduled = job.status === 'FAILED' && !!job.retry_after;
  const displayStatus = retryScheduled ? 'RETRYING' : job.status;
  const statusConfig = getStatusConfig(displayStatus);
  const StatusIcon = statusConfig.icon;
  // Workflow steps are retried through their workflow, so the job's own retry
  // endpoint is only offered for standalone jobs.
  const workflowRetryId = job.belongs_to_workflow ? job.pipeline_id : null;
  const inputSize = job.execution_info?.input_size_bytes
    ? formatBytes(job.execution_info.input_size_bytes, { decimals: 1 })
    : null;
  const outputSize = job.execution_info?.output_size_bytes
    ? formatBytes(job.execution_info.output_size_bytes, { decimals: 1 })
    : null;

  return (
    <div className="relative min-h-screen bg-background overflow-hidden selection:bg-primary/20">
      <AmbientBackground />
      {/* Columns follow this page's own width, which the app sidebar narrows without the
          viewport breakpoints noticing. */}
      <div className="@container relative z-10 max-w-7xl mx-auto px-6 py-8 pb-32">
        {/* Header */}
        <div className="flex flex-col gap-8 mb-10">
          <BackLink to="/pipeline/jobs" label={<Trans>Back to Jobs</Trans>} />

          <div className="flex flex-wrap items-start justify-between gap-6">
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.1 }}
              className="flex min-w-0 items-start gap-5"
            >
              <div
                className={cn(
                  'flex items-center justify-center w-16 h-16 rounded-2xl shadow-lg ring-1 ring-white/10 backdrop-blur-md bg-gradient-to-br shrink-0',
                  statusConfig.gradient,
                )}
              >
                <StatusIcon className={cn('h-8 w-8', statusConfig.textColor)} />
              </div>
              <div className="min-w-0">
                <div className="flex items-center gap-3 mb-1.5">
                  <h1 className="text-3xl font-bold tracking-tight text-foreground">
                    <Trans>Job Details</Trans>
                  </h1>
                  <Badge
                    variant="outline"
                    className={cn(
                      'border bg-background/50 backdrop-blur font-mono text-xs uppercase tracking-wider h-6',
                      statusConfig.borderColor,
                      statusConfig.textColor,
                    )}
                  >
                    {getStatusLabel(i18n, displayStatus)}
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
                  <JobAttempt job={job} />
                  <span className="opacity-40">•</span>
                  <span className="font-mono text-xs opacity-70 selection:bg-muted select-all break-all">
                    <Trans>ID: {jobId}</Trans>
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
              {/* The backend refuses a workflow retry while the step's own
                  automatic retry is pending. */}
              {['FAILED', 'CANCELLED'].includes(job.status) &&
                !retryScheduled &&
                (workflowRetryId ? (
                  <Button
                    className="bg-primary shadow-lg shadow-primary/20 hover:shadow-primary/40 transition-all font-medium"
                    onClick={() =>
                      retryWorkflowMutation.mutate(workflowRetryId)
                    }
                    disabled={retryWorkflowMutation.isPending}
                    title={i18n._(
                      msg`This job is a workflow step, so it restarts together with the rest of the workflow.`,
                    )}
                  >
                    <RotateCcw
                      className={cn(
                        'mr-2 h-4 w-4',
                        retryWorkflowMutation.isPending && 'animate-spin',
                      )}
                    />
                    <Trans>Retry Workflow</Trans>
                  </Button>
                ) : (
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
                ))}
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
              {['COMPLETED', 'FAILED', 'CANCELLED'].includes(job.status) && (
                <Button
                  variant="destructive"
                  className="shadow-lg shadow-destructive/20 hover:shadow-destructive/40 transition-all"
                  onClick={() => setConfirmDeleteOpen(true)}
                  disabled={deleteMutation.isPending}
                >
                  <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete Job</Trans>
                </Button>
              )}
              <ConfirmDialog
                open={confirmDeleteOpen}
                onOpenChange={setConfirmDeleteOpen}
                title={<Trans>Delete this job?</Trans>}
                description={
                  <Trans>
                    This permanently removes the job and its logs. This cannot
                    be undone.
                  </Trans>
                }
                onConfirm={() => deleteMutation.mutate(job.id)}
              />
            </motion.div>
          </div>
        </div>

        {/* Content Grid */}
        <div className="grid grid-cols-1 @3xl:grid-cols-3 gap-6">
          {/* Left Column: Metadata & Config */}
          <div className="min-w-0 space-y-6 @3xl:col-span-2">
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
                <CardContent className="p-6 grid grid-cols-1 @xl:grid-cols-2 gap-8">
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
                      inputSize || outputSize ? (
                        <div className="flex flex-wrap gap-2">
                          {inputSize ? (
                            <Badge variant="secondary" className="text-[10px]">
                              <Trans>In: {inputSize}</Trans>
                            </Badge>
                          ) : null}
                          {outputSize ? (
                            <Badge variant="secondary" className="text-[10px]">
                              <Trans>Out: {outputSize}</Trans>
                            </Badge>
                          ) : null}
                        </div>
                      ) : null
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
              <div className="grid grid-cols-1 @xl:grid-cols-2 gap-6">
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

            {uploadRecords.length > 0 && (
              <motion.div
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: 0.45 }}
              >
                <JobUploadsCard records={uploadRecords} />
              </motion.div>
            )}
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
                <LiveJobProgressPanel
                  job={job}
                  live={showLiveProgress}
                  polledSnapshot={polledSnapshot}
                />
                <Separator className="bg-border/50" />
                <div className="space-y-6">
                  <TimelineItem
                    label={i18n._(msg`Created`)}
                    time={formatDate(i18n.locale, job.created_at, {
                      timeStyle: 'medium',
                    })}
                    active
                  />
                  <TimelineItem
                    label={i18n._(msg`Started`)}
                    time={
                      job.started_at
                        ? formatDate(i18n.locale, job.started_at, {
                            timeStyle: 'medium',
                          })
                        : undefined
                    }
                    active={!!job.started_at}
                  />
                  <TimelineItem
                    label={i18n._(msg`Completed`)}
                    time={
                      job.completed_at
                        ? formatDate(i18n.locale, job.completed_at, {
                            timeStyle: 'medium',
                          })
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
          <div className="min-w-0 @3xl:col-span-3 space-y-6">
            <StalledOperationBanner
              jobId={job.id}
              enabled={job.status === 'FAILED' || job.status === 'CANCELLED'}
            />
            <JobInterruptedOutputsCard jobId={job.id} />
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
                    <div className="font-mono text-sm text-destructive/90 bg-background/50 p-4 rounded-lg border border-destructive/20 selection:bg-destructive/20 wrap-break-word">
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
                <JobLogViewer
                  jobId={job.id}
                  data={logsData}
                  isLoading={isLogsLoading}
                  hasNextPage={hasNextPage}
                  isFetchingNextPage={isFetchingNextPage}
                  fetchNextPage={fetchNextPage}
                  errorCount={job.execution_info.log_error_count}
                  warnCount={job.execution_info.log_warn_count}
                  linesTotal={job.execution_info.log_lines_total}
                />
              </motion.div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Which attempt a job that has run more than once is on and, while a failed
 * workflow step waits for its automatic retry, when the next attempt starts.
 */
function JobAttempt({ job }: { job: Job }) {
  const retryAt = job.status === 'FAILED' ? job.retry_after : null;

  // While a retry waits, the attempt shown is the one it will start.
  const attempt = job.retry_count + (retryAt ? 2 : 1);
  if (attempt === 1) return null;
  // A manual workflow retry can run a step past its automatic budget.
  const maxAttempts =
    job.max_attempts != null && attempt <= job.max_attempts
      ? job.max_attempts
      : null;

  return (
    <>
      <span className="opacity-40">•</span>
      <span
        className={cn(
          'px-2 py-0.5 rounded-md border tabular-nums',
          retryAt
            ? getStatusConfig('RETRYING').bgColor
            : 'bg-muted/50 border-border/50',
        )}
      >
        {retryAt && <RetryCountdown retryAt={Date.parse(retryAt)} />}
        {retryAt && ' · '}
        {maxAttempts ? (
          <Trans>
            Attempt {attempt} of {maxAttempts}
          </Trans>
        ) : (
          <Trans>Attempt {attempt}</Trans>
        )}
      </span>
    </>
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

/**
 * The job's progress panel, fed from the live-updates socket while `live`,
 * otherwise from the polled snapshot.
 */
function LiveJobProgressPanel({
  job,
  live,
  polledSnapshot,
}: {
  job: Job;
  live: boolean;
  polledSnapshot?: JobProgressSnapshot;
}) {
  const liveView = useUploadStore((state) =>
    live
      ? (state.uploadsByJobId.get(job.id) ?? state.jobsByJobId.get(job.id))
      : undefined,
  );
  const startedAtMs = job.started_at ? Date.parse(job.started_at) : 0;
  const liveSnapshot = useMemo(
    () => liveProgressSnapshot(liveView, startedAtMs),
    [liveView, startedAtMs],
  );
  return (
    <JobProgressPanel job={job} snapshot={liveSnapshot ?? polledSnapshot} />
  );
}
