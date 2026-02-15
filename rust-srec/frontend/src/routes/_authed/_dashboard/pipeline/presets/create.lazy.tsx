import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createJobPreset } from '@/server/functions/job';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { PresetEditor } from '@/components/pipeline/presets/preset-editor';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/presets/create',
)({
  component: CreatePresetPage,
});

function CreatePresetPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const createMutation = useMutation({
    mutationFn: createJobPreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Preset created successfully`));
      void queryClient.invalidateQueries({ queryKey: ['job', 'presets'] });
      void navigate({ to: '/pipeline/presets' });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to create preset: ${error.message}`)),
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
