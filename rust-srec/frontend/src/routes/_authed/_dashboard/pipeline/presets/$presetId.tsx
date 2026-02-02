import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { getJobPreset, updateJobPreset } from '@/server/functions/job';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { PresetEditor } from '@/components/pipeline/presets/preset-editor';
import { Skeleton } from '@/components/ui/skeleton';

export const Route = createFileRoute(
  '/_authed/_dashboard/pipeline/presets/$presetId',
)({
  component: EditPresetPage,
});

function EditPresetPage() {
  const { presetId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const {
    data: preset,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['job', 'preset', presetId],
    queryFn: () => getJobPreset({ data: presetId }),
  });

  const updateMutation = useMutation({
    mutationFn: updateJobPreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Preset updated successfully`));
      void queryClient.invalidateQueries({ queryKey: ['job', 'presets'] });
      void queryClient.invalidateQueries({
        queryKey: ['job', 'preset', presetId],
      });
      void navigate({ to: '/pipeline/presets' });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to update preset: ${error.message}`)),
  });

  const onSubmit = (data: any) => {
    updateMutation.mutate({ data });
  };

  if (isLoading) {
    return (
      <div className="space-y-6 max-w-6xl mx-auto p-6 md:p-10">
        <div className="flex flex-col gap-4">
          <Skeleton className="h-10 w-1/3" />
          <Skeleton className="h-6 w-1/2" />
        </div>
        <div className="grid grid-cols-1 md:grid-cols-12 gap-8">
          <div className="md:col-span-4">
            <Skeleton className="h-[400px] w-full rounded-xl" />
          </div>
          <div className="md:col-span-8">
            <Skeleton className="h-[600px] w-full rounded-xl" />
          </div>
        </div>
      </div>
    );
  }

  if (error || !preset) {
    return (
      <div className="flex flex-col items-center justify-center p-20 text-center">
        <h3 className="text-xl font-bold text-destructive">
          <Trans>Error loading preset</Trans>
        </h3>
        <p className="text-muted-foreground mt-2">
          {error?.message || i18n._(msg`Preset not found`)}
        </p>
      </div>
    );
  }

  return (
    <PresetEditor
      initialData={preset}
      title={<Trans>Edit Preset: {preset.name}</Trans>}
      onSubmit={onSubmit}
      isUpdating={updateMutation.isPending}
    />
  );
}
