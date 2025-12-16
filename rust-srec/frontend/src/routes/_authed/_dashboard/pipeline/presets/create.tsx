import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createJobPreset } from '@/server/functions/job';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { PresetEditor } from '@/components/pipeline/presets/preset-editor';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/presets/create',
)({
  component: CreatePresetPage,
});

function CreatePresetPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: createJobPreset,
    onSuccess: () => {
      toast.success(t`Preset created successfully`);
      queryClient.invalidateQueries({ queryKey: ['job', 'presets'] });
      navigate({ to: '/pipeline/presets' });
    },
    onError: (error) =>
      toast.error(t`Failed to create preset: ${error.message}`),
  });

  const onSubmit = (data: any) => {
    createMutation.mutate({ data });
  };

  return (
    <PresetEditor
      title={<Trans>Create New Preset</Trans>}
      onSubmit={onSubmit}
      isUpdating={createMutation.isPending}
    />
  );
}
