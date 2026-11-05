import { useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import {
  cancelActivePipelineJob,
  deletePipelineJob,
  retryDagSteps,
  retryPipelineJob,
} from '@/server/functions/pipeline';

/** The retry, cancel and delete actions of the job detail page. */
export function useJobActions(jobId: string) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  // Retrying or cancelling changes the job, not the rows already logged: a
  // retried job appends to them through the tail read, and a cancelled one
  // gets its closing rows from the terminal-state read in `useJobLogs`.
  const invalidateJob = () =>
    queryClient.invalidateQueries({
      queryKey: ['pipeline', 'job', jobId],
      exact: true,
    });

  const navigate = useNavigate();

  const retryMutation = useMutation({
    mutationFn: (id: string) => retryPipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Job retry initiated`));
      void invalidateJob();
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to retry job`), {
        description: error.message,
      }),
  });

  // A workflow step cannot be retried on its own: the backend restarts it together
  // with the branches that were cancelled alongside it.
  const retryWorkflowMutation = useMutation({
    mutationFn: (pipelineId: string) => retryDagSteps({ data: pipelineId }),
    onSuccess: (result) => {
      toast.success(i18n._(msg`Workflow retry initiated`));
      void invalidateJob();
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'executions', result.dag_id],
      });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to retry workflow`), {
        description: error.message,
      }),
  });

  const cancelMutation = useMutation({
    mutationFn: (id: string) => cancelActivePipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Job cancelled`));
      void invalidateJob();
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to cancel job`), {
        description: error.message,
      }),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deletePipelineJob({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Job deleted`));
      // Lists and workflow views that showed the job.
      for (const queryKey of [
        ['pipeline', 'pipelines'],
        ['pipeline', 'stats'],
        ['pipeline', 'executions'],
        ['pipeline', 'dags'],
      ]) {
        void queryClient.invalidateQueries({ queryKey });
      }
      // The job's own queries can no longer be read. They are dropped once
      // this page has unmounted; dropped earlier, its observers would read
      // the deleted job again.
      void navigate({ to: '/pipeline/jobs' }).then(() =>
        queryClient.removeQueries({ queryKey: ['pipeline', 'job', jobId] }),
      );
    },
    onError: () => toast.error(i18n._(msg`Failed to delete job`)),
  });

  return {
    retryMutation,
    retryWorkflowMutation,
    cancelMutation,
    deleteMutation,
  };
}
