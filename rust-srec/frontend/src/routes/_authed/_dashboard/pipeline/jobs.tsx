import { createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { getPipelineStats, listPipelineJobs, retryPipelineJob, cancelPipelineJob } from '@/server/functions';
import { Card, CardContent, CardHeader, CardTitle } from '../../../../components/ui/card';
import { Skeleton } from '../../../../components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Badge } from '../../../../components/ui/badge';
import { Button } from '../../../../components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from '../../../../components/ui/table';
import { RefreshCw, XCircle, RotateCcw, Play } from 'lucide-react';
import { toast } from 'sonner';
import { format } from 'date-fns';

export const Route = createFileRoute('/_authed/_dashboard/pipeline/jobs')({
  component: PipelineJobsPage,
});

function PipelineJobsPage() {
  const queryClient = useQueryClient();

  const { data: stats, isLoading: isStatsLoading } = useQuery({
    queryKey: ['pipeline', 'stats'],
    queryFn: () => getPipelineStats(),
    refetchInterval: 5000,
  });

  const { data: jobsData, isLoading: isJobsLoading } = useQuery({
    queryKey: ['pipeline', 'jobs'],
    queryFn: () => listPipelineJobs(), // TODO: Add filtering support
    refetchInterval: 5000,
  });

  const jobs = jobsData?.items || [];

  const retryMutation = useMutation({
    mutationFn: (id: string) => retryPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(t`Job retry initiated`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'jobs'] });
    },
    onError: () => toast.error(t`Failed to retry job`),
  });

  const cancelMutation = useMutation({
    mutationFn: (id: string) => cancelPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(t`Job cancelled`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'jobs'] });
    },
    onError: () => toast.error(t`Failed to cancel job`),
  });

  // Separate active and history
  const activeJobs = jobs?.filter(j => ['Pending', 'Processing'].includes(j.status)) || [];
  const historyJobs = jobs?.filter(j => !['Pending', 'Processing'].includes(j.status)) || [];

  return (
    <div className="space-y-6">


      {/* Stats Overview */}
      <div className="grid gap-4 md:grid-cols-4">
        <StatCard title={<Trans>Pending</Trans>} value={stats?.pending_count} loading={isStatsLoading} />
        <StatCard title={<Trans>Processing</Trans>} value={stats?.processing_count} loading={isStatsLoading} color="text-blue-500" />
        <StatCard title={<Trans>Completed</Trans>} value={stats?.completed_count} loading={isStatsLoading} color="text-green-500" />
        <StatCard title={<Trans>Failed</Trans>} value={stats?.failed_count} loading={isStatsLoading} color="text-red-500" />
      </div>

      {/* Active Jobs */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Play className="h-5 w-5 text-blue-500 animate-pulse" />
            <Trans>Active Jobs</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent>
          {isJobsLoading ? (
            <Skeleton className="h-24 w-full" />
          ) : activeJobs.length > 0 ? (
            <JobsTable
              jobs={activeJobs}
              onRetry={(id) => retryMutation.mutate(id)}
              onCancel={(id) => cancelMutation.mutate(id)}
            />
          ) : (
            <div className="text-center py-8 text-muted-foreground"><Trans>No active jobs</Trans></div>
          )}
        </CardContent>
      </Card>

      {/* History */}
      <Card>
        <CardHeader>
          <CardTitle><Trans>Job History</Trans></CardTitle>
        </CardHeader>
        <CardContent>
          {isJobsLoading ? (
            <Skeleton className="h-48 w-full" />
          ) : (
            <JobsTable
              jobs={historyJobs}
              onRetry={(id) => retryMutation.mutate(id)}
              // Cancel is usually only for active, but maybe for stuck ones?
              onCancel={(id) => cancelMutation.mutate(id)}
              isHistory
            />
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function StatCard({ title, value, loading, color }: { title: React.ReactNode, value?: number, loading: boolean, color?: string }) {
  return (
    <Card className="p-4 flex flex-col justify-between">
      <p className="text-sm font-medium text-muted-foreground">{title}</p>
      {loading ? <Skeleton className="h-8 w-10 mt-2" /> : <p className={`text-2xl font-bold mt-2 ${color}`}>{value ?? 0}</p>}
    </Card>
  )
}

function JobsTable({ jobs, onRetry, onCancel, isHistory = false }: { jobs: any[], onRetry: (id: string) => void, onCancel: (id: string) => void, isHistory?: boolean }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead><Trans>Job ID</Trans></TableHead>
          <TableHead><Trans>Step</Trans></TableHead>
          <TableHead><Trans>Status</Trans></TableHead>
          <TableHead><Trans>Progress</Trans></TableHead>
          <TableHead><Trans>Updated</Trans></TableHead>
          <TableHead className="text-right"><Trans>Actions</Trans></TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {jobs.map(job => (
          <TableRow key={job.id}>
            <TableCell className="font-mono text-xs">{job.id.substring(0, 8)}...</TableCell>
            <TableCell>{job.step}</TableCell>
            <TableCell><JobStatusBadge status={job.status} /></TableCell>
            <TableCell>{job.progress !== undefined ? `${job.progress}%` : `-`}</TableCell>
            <TableCell className="text-xs text-muted-foreground">
              {job.started_at ? format(new Date(job.started_at), 'HH:mm:ss') : '-'}
            </TableCell>
            <TableCell className="text-right">
              {job.status === 'Failed' && (
                <Button variant="ghost" size="icon" onClick={() => onRetry(job.id)} title={t`Retry`}>
                  <RotateCcw className="h-4 w-4" />
                </Button>
              )}
              {['Pending', 'Processing'].includes(job.status) && !isHistory && (
                <Button variant="ghost" size="icon" onClick={() => onCancel(job.id)} title={t`Cancel`} className="text-red-500 hover:text-red-600">
                  <XCircle className="h-4 w-4" />
                </Button>
              )}
            </TableCell>
          </TableRow>
        ))
        }
      </TableBody >
    </Table >
  )
}

function JobStatusBadge({ status }: { status: string }) {
  let variant: "default" | "secondary" | "destructive" | "outline" = "secondary";
  if (['Processing'].includes(status)) variant = 'default';
  if (['Failed', 'Cancelled'].includes(status)) variant = 'destructive';

  return <Badge variant={variant}>{status}</Badge>;
}
