import { Button } from '@/components/ui/button';
import { Link } from '@tanstack/react-router';
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import {
  MoreHorizontal,
  RotateCcw,
  XCircle,
  Clock,
  RefreshCw,
  CheckCircle2,
  AlertCircle,
  Timer,
  ArrowRight,
  ExternalLink,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { formatDistanceToNow } from 'date-fns';
import { z } from 'zod';
import { JobSchema } from '@/api/schemas';
import { getStepIcon, getStepColor } from '../constants';

type Job = z.infer<typeof JobSchema>;

interface JobCardProps {
  jobs: Job[];
  onRetry: (id: string) => void;
  onCancel: (id: string) => void;
  onCancelPipeline?: (pipelineId: string) => void;
  onViewDetails: (job: Job) => void;
  onViewJob?: (job: Job) => void;
}

// Helper function to format duration in human-readable format
function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || seconds === 0) return '-';

  if (seconds < 1) {
    return `${Math.round(seconds * 1000)}ms`;
  } else if (seconds < 60) {
    return `${seconds.toFixed(1)}s`;
  } else if (seconds < 3600) {
    const mins = Math.floor(seconds / 60);
    const secs = Math.round(seconds % 60);
    return `${mins}m ${secs}s`;
  } else {
    const hours = Math.floor(seconds / 3600);
    const mins = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${mins}m`;
  }
}

const JobStatus = {
  Pending: 'PENDING',
  Processing: 'PROCESSING',
  Completed: 'COMPLETED',
  Failed: 'FAILED',
  Cancelled: 'CANCELLED',
  Interrupted: 'INTERRUPTED',
} as const;

interface StatusConfigItem {
  icon: React.ElementType;
  color: string;
  badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline';
  animate?: boolean;
}

const STATUS_CONFIG: Record<string, StatusConfigItem> = {
  [JobStatus.Pending]: {
    icon: Clock,
    color: 'bg-muted text-muted-foreground',
    badgeVariant: 'secondary',
  },
  [JobStatus.Processing]: {
    icon: RefreshCw,
    color: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
    badgeVariant: 'default',
    animate: true,
  },
  [JobStatus.Completed]: {
    icon: CheckCircle2,
    color: 'bg-green-500/10 text-green-500 border-green-500/20',
    badgeVariant: 'secondary',
  },
  [JobStatus.Failed]: {
    icon: XCircle,
    color: 'bg-red-500/10 text-red-500 border-red-500/20',
    badgeVariant: 'destructive',
  },
  [JobStatus.Cancelled]: {
    icon: AlertCircle,
    color: 'bg-gray-500/10 text-gray-500 border-gray-500/20',
    badgeVariant: 'secondary',
  },
  [JobStatus.Interrupted]: {
    icon: AlertCircle,
    color: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
    badgeVariant: 'secondary',
  },
};

export function JobCard({
  jobs,
  onRetry,
  onCancel,
  onCancelPipeline,
  onViewDetails,
  onViewJob,
}: JobCardProps) {
  // Sort jobs by created_at to ensure correct order step 1 -> step 2
  const sortedJobs = [...jobs].sort(
    (a, b) =>
      new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
  );
  const firstJob = sortedJobs[0];

  // Overall status
  const isFailed = sortedJobs.some((j) => j.status === JobStatus.Failed);
  const isProcessing = sortedJobs.some(
    (j) => j.status === JobStatus.Processing,
  );
  const isCompleted = sortedJobs.every((j) => j.status === JobStatus.Completed);
  const isPending = sortedJobs.every((j) => j.status === JobStatus.Pending);

  // Determine overall status
  const overallStatus = isFailed
    ? JobStatus.Failed
    : isProcessing
      ? JobStatus.Processing
      : isCompleted
        ? JobStatus.Completed
        : isPending
          ? JobStatus.Pending
          : JobStatus.Processing;
  const statusConfig = STATUS_CONFIG[overallStatus];
  const StatusIcon = statusConfig.icon;

  // Calculate total duration for completed pipelines
  const totalDuration = sortedJobs.reduce(
    (acc, job) => acc + (job.duration_secs || 0),
    0,
  );

  // Determine the "primary" job to navigate to when clicking the card body
  // Priority: Failed -> Processing -> Last Created (usually current/last step)
  const failedJob = sortedJobs.find((j) => j.status === JobStatus.Failed);
  const processingJob = sortedJobs.find(
    (j) => j.status === JobStatus.Processing,
  );
  const lastJob = sortedJobs[sortedJobs.length - 1];
  const primaryJob = failedJob || processingJob || lastJob || firstJob;

  return (
    <Card
      onClick={() => onViewDetails(primaryJob)}
      className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20 cursor-pointer"
    >
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
        <div
          className={`p-3 rounded-2xl ${statusConfig.color} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3`}
        >
          <StatusIcon
            className={`h-5 w-5 ${statusConfig.animate ? 'animate-spin' : ''}`}
          />
        </div>
        <div className="flex-1 min-w-0 space-y-1">
          <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
            <Link
              to="/pipeline/executions/$pipelineId"
              params={{ pipelineId: primaryJob.pipeline_id || '' }}
              disabled={!primaryJob.pipeline_id}
              className="hover:underline underline-offset-4 decoration-primary/50"
              onClick={(e: React.MouseEvent) => e.stopPropagation()}
            >
              {primaryJob.pipeline_id
                ? `PIPE-${primaryJob.pipeline_id.substring(0, 8)}`
                : `JOB-${primaryJob.id.substring(0, 8)}`}
            </Link>
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
              {primaryJob.streamer_id}
            </span>
          </div>
        </div>
        <Badge variant={statusConfig.badgeVariant} className="capitalize">
          {overallStatus.toLowerCase()}
        </Badge>
        <DropdownMenu>
          <DropdownMenuTrigger asChild onClick={(e) => e.stopPropagation()}>
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors"
            >
              <MoreHorizontal className="h-4 w-4" />
              <span className="sr-only">
                <Trans>Open menu</Trans>
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem
              onClick={(e) => {
                e.stopPropagation();
                onViewDetails(primaryJob);
              }}
            >
              <ExternalLink className="mr-2 h-4 w-4" />{' '}
              <Trans>View Details</Trans>
            </DropdownMenuItem>
            {isFailed && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={(e) => {
                    e.stopPropagation(); // Ensure we don't trigger card click
                    const failedJob = sortedJobs.find(
                      (j) => j.status === JobStatus.Failed,
                    );
                    if (failedJob) onRetry(failedJob.id);
                  }}
                >
                  <RotateCcw className="mr-2 h-4 w-4" />{' '}
                  <Trans>Retry Failed</Trans>
                </DropdownMenuItem>
              </>
            )}
            {(isPending || isProcessing) && (
              <>
                <DropdownMenuSeparator />
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <DropdownMenuItem
                      className="text-destructive focus:text-destructive"
                      onSelect={(e) => e.preventDefault()}
                    >
                      <XCircle className="mr-2 h-4 w-4" /> <Trans>Cancel</Trans>
                    </DropdownMenuItem>
                  </AlertDialogTrigger>
                  <AlertDialogContent>
                    <AlertDialogHeader>
                      <AlertDialogTitle>
                        <Trans>Cancel Pipeline?</Trans>
                      </AlertDialogTitle>
                      <AlertDialogDescription>
                        <Trans>
                          This will cancel all pending and processing jobs in
                          this pipeline.
                        </Trans>
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>
                        <Trans>Keep Running</Trans>
                      </AlertDialogCancel>
                      <AlertDialogAction
                        onClick={() => {
                          // Use pipeline cancellation if available
                          const pipelineId = firstJob.pipeline_id;
                          if (pipelineId && onCancelPipeline) {
                            onCancelPipeline(pipelineId);
                          } else {
                            // Fallback: cancel individual jobs
                            sortedJobs.forEach((job) => {
                              const cancelableStatuses: string[] = [
                                JobStatus.Pending,
                                JobStatus.Processing,
                              ];
                              if (cancelableStatuses.includes(job.status)) {
                                onCancel(job.id);
                              }
                            });
                          }
                        }}
                        className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                      >
                        <Trans>Cancel</Trans>
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </CardHeader>

      <CardContent className="relative pb-4 flex-1 z-10">
        <p className="text-xs text-muted-foreground/80 mb-4 leading-relaxed font-light">
          <Trans>Started</Trans>{' '}
          {formatDistanceToNow(new Date(primaryJob.created_at), {
            addSuffix: true,
          })}
          {primaryJob.session_id &&
            ` - Session: ${primaryJob.session_id.substring(0, 8)}`}
        </p>

        {/* Pipeline Steps Visualization */}
        <div className="flex items-center gap-1 flex-wrap">
          {sortedJobs.map((job, index) => {
            const StepIcon = getStepIcon(job.processor_type);
            const colorClass = getStepColor(job.processor_type);
            const jobStatusConfig =
              STATUS_CONFIG[job.status] || STATUS_CONFIG[JobStatus.Pending];
            const JobStatusIcon = jobStatusConfig.icon;

            return (
              <div key={job.id} className="flex items-center">
                <div
                  className={`relative flex items-center gap-1.5 px-2 py-1 rounded-md bg-gradient-to-br ${colorClass} border transition-all group-hover:scale-105 cursor-pointer hover:ring-2 hover:ring-primary/20`}
                  title={`${job.processor_type}: ${job.status}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (onViewJob) {
                      onViewJob(job);
                    } else {
                      onViewDetails(job);
                    }
                  }}
                >
                  <StepIcon className="h-3 w-3" />
                  <span className="text-[10px] font-medium truncate max-w-[60px] uppercase">
                    {job.processor_type}
                  </span>
                  {/* Status indicator */}
                  <div
                    className={`absolute -top-1 -right-1 flex h-4 w-4 items-center justify-center rounded-full ${jobStatusConfig.color} border shadow-sm`}
                  >
                    <JobStatusIcon
                      className={`h-2.5 w-2.5 ${jobStatusConfig.animate ? 'animate-spin' : ''}`}
                    />
                  </div>
                </div>
                {index < sortedJobs.length - 1 && (
                  <ArrowRight className="h-3 w-3 mx-1 text-muted-foreground/30" />
                )}
              </div>
            );
          })}
        </div>

        {/* Duration Info */}
        {isCompleted && totalDuration > 0 && (
          <div className="flex items-center gap-2 mt-3 text-xs text-muted-foreground">
            <Timer className="h-3 w-3" />
            <span>
              <Trans>Total time:</Trans> {formatDuration(totalDuration)}
            </span>
          </div>
        )}

        {/* Error Message Preview */}
        {isFailed && (
          <div className="mt-3 p-2 rounded-md bg-red-500/10 border border-red-500/20">
            <p className="text-[10px] text-red-500 line-clamp-2">
              {sortedJobs.find((j) => j.status === JobStatus.Failed)
                ?.error_message || <Trans>Processing failed</Trans>}
            </p>
          </div>
        )}
      </CardContent>

      <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
        <span className="font-mono opacity-50">
          {sortedJobs.length} {sortedJobs.length === 1 ? 'step' : 'steps'}
        </span>
        {isCompleted && totalDuration > 0 && (
          <span className="font-mono opacity-50">
            {formatDuration(totalDuration)}
          </span>
        )}
      </CardFooter>
    </Card>
  );
}
