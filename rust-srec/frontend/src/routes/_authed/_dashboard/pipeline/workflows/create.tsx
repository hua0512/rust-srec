import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createPipelinePreset } from '@/server/functions/pipeline';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { WorkflowEditor } from '@/components/pipeline/workflows/workflow-editor';
import { DagStepDefinition } from '@/api/schemas';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/workflows/create',
)({
  component: CreateWorkflowPage,
});

function CreateWorkflowPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const createMutation = useMutation({
    mutationFn: createPipelinePreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Workflow created successfully`));
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'workflows'],
      });
      void navigate({ to: '/pipeline/workflows' });
    },
    onError: (error) => {
      console.error('Failed to create workflow:', error);
      toast.error(i18n._(msg`Failed to create workflow: ${error.message}`));
    },
  });

  const onSubmit = (data: {
    name: string;
    description?: string;
    steps: DagStepDefinition[];
  }) => {
    console.log(data);
    createMutation.mutate({
      data: {
        name: data.name,
        description: data.description,
        dag: {
          name: data.name,
          steps: data.steps,
        },
      },
    });
  };

  return (
    <WorkflowEditor
      title={<Trans>Create New Workflow</Trans>}
      onSubmit={onSubmit}
      isUpdating={createMutation.isPending}
    />
  );
}
