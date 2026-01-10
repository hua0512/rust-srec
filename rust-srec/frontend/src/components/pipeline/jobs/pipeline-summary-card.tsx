import { memo } from 'react';
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
  XCircle,
  CheckCircle2,
  Timer,
  ExternalLink,
  Layers,
  Trash2,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { type DagSummary } from '@/api/schemas';

import { formatRelativeTime } from '@/lib/date-utils';
import { plural } from '@lingui/core/macro';
import { STATUS_CONFIG } from '@/components/pipeline/status-config';

interface PipelineSummaryCardProps {
  pipeline: DagSummary;
  onCancelPipeline?: (pipelineId: string) => void;
  onDeletePipeline?: (pipelineId: string) => void;
  onViewDetails: (pipelineId: string) => void;
}

// Custom comparison function to prevent unnecessary re-renders
function arePropsEqual(
  prevProps: PipelineSummaryCardProps,
  nextProps: PipelineSummaryCardProps,
): boolean {
  const prevPipeline = prevProps.pipeline;
  const nextPipeline = nextProps.pipeline;

  return (
    prevPipeline.id === nextPipeline.id &&
    prevPipeline.status === nextPipeline.status &&
    prevPipeline.progress_percent === nextPipeline.progress_percent &&
    prevPipeline.completed_steps === nextPipeline.completed_steps &&
    prevPipeline.failed_steps === nextPipeline.failed_steps &&
    prevPipeline.total_steps === nextPipeline.total_steps &&
    prevPipeline.created_at === nextPipeline.created_at &&
    prevProps.onCancelPipeline === nextProps.onCancelPipeline &&
    prevProps.onDeletePipeline === nextProps.onDeletePipeline &&
    prevProps.onViewDetails === nextProps.onViewDetails
  );
}

export const PipelineSummaryCard = memo(function PipelineSummaryCard({
  pipeline,
  onCancelPipeline,
  onDeletePipeline,
  onViewDetails,
}: PipelineSummaryCardProps) {
  const { i18n } = useLingui();
  const statusKey = pipeline.status;
  const statusConfig = STATUS_CONFIG[statusKey] || STATUS_CONFIG.MIXED;
  const StatusIcon = statusConfig.icon;

  const isCompleted = statusKey === 'COMPLETED';
  const isFailed = statusKey === 'FAILED';
  const isPending = statusKey === 'PENDING';
  const isProcessing = statusKey === 'PROCESSING';
  // Show cancel button if there are any jobs that aren't completed or failed
  const hasUnfinishedJobs =
    pipeline.total_steps > pipeline.completed_steps + pipeline.failed_steps;
  const canCancel = isPending || isProcessing || hasUnfinishedJobs;
  const statusLabel = statusKey.toLowerCase();

  return (
    <Card
      onClick={() => onViewDetails(pipeline.id)}
      className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20 cursor-pointer"
    >
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-3 sm:gap-4 p-3 sm:pb-2 sm:space-y-0 z-10">
        <div
          className={`p-2.5 sm:p-3 rounded-xl sm:rounded-2xl ${statusConfig.bgColor} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3 shrink-0`}
        >
          <StatusIcon
            className={`h-4 w-4 sm:h-5 sm:w-5 ${statusConfig.animate ? 'animate-spin' : ''}`}
          />
        </div>
        <div className="flex-1 min-w-0 space-y-1">
          <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
            <Link
              to="/pipeline/executions/$pipelineId"
              params={{ pipelineId: pipeline.id }}
              className="hover:underline underline-offset-4 decoration-primary/50"
              onClick={(e: React.MouseEvent) => e.stopPropagation()}
            >
              PIPE-{pipeline.id.substring(0, 8)}
            </Link>
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
              {pipeline.streamer_name ?? pipeline.name ?? pipeline.streamer_id}
            </span>
          </div>
        </div>
        <Badge
          variant={statusConfig.badgeVariant}
          className="capitalize py-0 h-6 text-[10px] sm:text-xs"
        >
          {statusLabel}
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
                onViewDetails(pipeline.id);
              }}
            >
              <ExternalLink className="mr-2 h-4 w-4" />{' '}
              <Trans>View Details</Trans>
            </DropdownMenuItem>
            {canCancel && onCancelPipeline && (
              <>
                <DropdownMenuSeparator />
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <DropdownMenuItem
                      className="text-destructive focus:text-destructive"
                      onSelect={(e) => e.preventDefault()}
                      onClick={(e) => e.stopPropagation()}
                    >
                      <XCircle className="mr-2 h-4 w-4" /> <Trans>Cancel</Trans>
                    </DropdownMenuItem>
                  </AlertDialogTrigger>
                  <AlertDialogContent onClick={(e) => e.stopPropagation()}>
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
                        onClick={() => onCancelPipeline(pipeline.id)}
                        className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                      >
                        <Trans>Cancel</Trans>
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              </>
            )}

            {/* Delete option for terminal states */}
            {(isCompleted || isFailed) && onDeletePipeline && (
              <>
                <DropdownMenuSeparator />
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <DropdownMenuItem
                      className="text-destructive focus:text-destructive"
                      onSelect={(e) => e.preventDefault()}
                      onClick={(e) => e.stopPropagation()}
                    >
                      <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                    </DropdownMenuItem>
                  </AlertDialogTrigger>
                  <AlertDialogContent onClick={(e) => e.stopPropagation()}>
                    <AlertDialogHeader>
                      <AlertDialogTitle>
                        <Trans>Delete Pipeline?</Trans>
                      </AlertDialogTitle>
                      <AlertDialogDescription>
                        <Trans>
                          This will permanently delete the pipeline and all its
                          associated jobs and logs. This action cannot be
                          undone.
                        </Trans>
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>
                        <Trans>Cancel</Trans>
                      </AlertDialogCancel>
                      <AlertDialogAction
                        onClick={() => onDeletePipeline(pipeline.id)}
                        className="bg-destructive text-white hover:bg-destructive/90"
                      >
                        <Trans>Delete</Trans>
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
        <p className="text-[10px] sm:text-xs text-muted-foreground/80 mb-3 sm:mb-4 leading-relaxed font-light truncate">
          <Trans>Started</Trans>{' '}
          {formatRelativeTime(new Date(pipeline.created_at), i18n.locale)}
          {pipeline.session_id && (
            <Trans> - Session: {pipeline.session_id.substring(0, 8)}</Trans>
          )}
        </p>

        {/* Job Counts */}
        <div className="flex items-center gap-3 flex-wrap">
          <div className="flex items-center gap-1.5 px-2 py-1 rounded-md bg-muted/50 border">
            <Layers className="h-3 w-3 text-muted-foreground" />
            <span className="text-[10px] font-medium">
              {plural(pipeline.total_steps, {
                one: '# step',
                other: '# steps',
              })}
            </span>
          </div>
          {pipeline.completed_steps > 0 && (
            <div className="flex items-center gap-1.5 px-2 py-1 rounded-md bg-green-500/10 border border-green-500/20">
              <CheckCircle2 className="h-3 w-3 text-green-500" />
              <span className="text-[10px] font-medium text-green-600 dark:text-green-400">
                {plural(pipeline.completed_steps, {
                  one: '# done',
                  other: '# done',
                })}
              </span>
            </div>
          )}
          {pipeline.failed_steps > 0 && (
            <div className="flex items-center gap-1.5 px-2 py-1 rounded-md bg-red-500/10 border border-red-500/20">
              <XCircle className="h-3 w-3 text-red-500" />
              <span className="text-[10px] font-medium text-red-600 dark:text-red-400">
                {plural(pipeline.failed_steps, {
                  one: '# failed',
                  other: '# failed',
                })}
              </span>
            </div>
          )}
        </div>

        <div className="flex items-center gap-2 mt-3 text-xs text-muted-foreground">
          <Timer className="h-3 w-3" />
          <span>
            <Trans>Progress:</Trans>{' '}
            {(pipeline.progress_percent || 0).toFixed(1)}%
          </span>
        </div>

        {/* Failed indicator */}
        {isFailed && (
          <div className="mt-3 p-2 rounded-md bg-red-500/10 border border-red-500/20">
            <p className="text-[10px] text-red-500 line-clamp-2">
              <Trans>Pipeline has failed jobs. Click to view details.</Trans>
            </p>
          </div>
        )}
      </CardContent>

      <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
        <span className="font-mono opacity-50">
          {plural(pipeline.total_steps, {
            one: '# step',
            other: '# steps',
          })}
        </span>
        <span className="font-mono opacity-50">
          {(pipeline.progress_percent || 0).toFixed(1)}%
        </span>
      </CardFooter>
    </Card>
  );
}, arePropsEqual);
