import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createPipelinePreset } from '@/server/functions/pipeline';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { WorkflowEditor } from '@/components/pipeline/workflows/workflow-editor';
import { PipelineStepSchema } from '@/api/schemas';
import { z } from 'zod';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/workflows/create',
)({
  component: CreateWorkflowPage,
});

function CreateWorkflowPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: createPipelinePreset,
    onSuccess: () => {
      toast.success(t`Workflow created successfully`);
      queryClient.invalidateQueries({ queryKey: ['pipeline', 'workflows'] });
      navigate({ to: '/pipeline/workflows' });
    },
    onError: (error) =>
      toast.error(t`Failed to create workflow: ${error.message}`),
  });

  const onSubmit = (data: {
    name: string;
    description?: string;
    steps: z.infer<typeof PipelineStepSchema>[];
  }) => {
    createMutation.mutate({ data });
  };

  return (
    <WorkflowEditor
      title={<Trans>Create New Workflow</Trans>}
      onSubmit={onSubmit}
      isUpdating={createMutation.isPending}
    />
  );
}
