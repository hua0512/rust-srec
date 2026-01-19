import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  getPipelinePreset,
  updatePipelinePreset,
} from '@/server/functions/pipeline';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { WorkflowEditor } from '@/components/pipeline/workflows/workflow-editor';
import { Skeleton } from '@/components/ui/skeleton';
import type { DagStepDefinition } from '@/api/schemas';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/workflows/$workflowId',
)({
  component: EditWorkflowPage,
});

function EditWorkflowPage() {
  const { workflowId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const {
    data: workflow,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['pipeline', 'workflow', workflowId],
    queryFn: () => getPipelinePreset({ data: workflowId }),
  });

  const updateMutation = useMutation({
    mutationFn: updatePipelinePreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Workflow updated successfully`));
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'workflows'] });
      queryClient.invalidateQueries({
        queryKey: ['pipeline', 'workflow', workflowId],
      });
      navigate({ to: '/pipeline/workflows' });
    },
    onError: (error) => {
      console.error('Failed to update workflow:', error);
      toast.error(i18n._(msg`Failed to update workflow: ${error.message}`));
    },
  });

  const onSubmit = (data: {
    name: string;
    description?: string;
    steps: DagStepDefinition[];
  }) => {
    console.log(data);
    updateMutation.mutate({
      data: {
        id: workflowId,
        data: {
          name: data.name,
          description: data.description,
          dag: {
            name: data.name,
            steps: data.steps,
          },
        },
      },
    });
  };

  if (isLoading) {
    return (
      <div className="space-y-6 max-w-5xl mx-auto p-6 md:p-10">
        <div className="flex flex-col gap-4">
          <Skeleton className="h-10 w-1/3" />
          <Skeleton className="h-6 w-1/2" />
        </div>
        <Skeleton className="h-[200px] w-full rounded-xl" />
        <Skeleton className="h-[300px] w-full rounded-xl" />
      </div>
    );
  }

  if (error || !workflow) {
    return (
      <div className="flex flex-col items-center justify-center p-20 text-center">
        <h3 className="text-xl font-bold text-destructive">
          <Trans>Error loading workflow</Trans>
        </h3>
        <p className="text-muted-foreground mt-2">
          {error?.message || i18n._(msg`Workflow not found`)}
        </p>
      </div>
    );
  }

  return (
    <WorkflowEditor
      initialData={workflow}
      title={<Trans>Edit Workflow: {workflow.name}</Trans>}
      onSubmit={onSubmit}
      isUpdating={updateMutation.isPending}
    />
  );
}
