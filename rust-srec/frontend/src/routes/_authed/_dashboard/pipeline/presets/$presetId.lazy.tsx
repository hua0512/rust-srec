import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import { getJobPreset, updateJobPreset } from '@/server/functions/job';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import {
  PresetEditor,
  type PresetFormValues,
} from '@/components/pipeline/presets/preset-editor';
import { Skeleton } from '@/components/ui/skeleton';
import { DetailLoadError } from '@/components/shared/detail-page';

export const Route = createLazyFileRoute(
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
    queryKey: queryKeys.jobPresets.detail(presetId),
    queryFn: () => getJobPreset({ data: presetId }),
  });

  const updateMutation = useMutation({
    mutationFn: updateJobPreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Preset updated successfully`));
      void queryClient.invalidateQueries({
        queryKey: queryKeys.jobPresets.all,
      });
      void navigate({ to: '/pipeline/presets' });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to update preset: ${error.message}`)),
  });

  const onSubmit = (data: PresetFormValues) => {
    updateMutation.mutate({ data });
  };

  if (isLoading) {
    return (
      <div className="space-y-6 max-w-6xl mx-auto p-6 md:p-10">
        <div className="flex flex-col gap-4">
          <Skeleton className="h-10 w-1/3" />
          <Skeleton className="h-6 w-1/2" />
        </div>
        {/* Mirrors the editor's columns, which follow its own width. */}
        <div className="@container">
          <div className="grid grid-cols-1 @3xl:grid-cols-12 gap-6 @3xl:gap-8">
            <div className="@3xl:col-span-4">
              <Skeleton className="h-[400px] w-full rounded-xl" />
            </div>
            <div className="@3xl:col-span-8">
              <Skeleton className="h-[600px] w-full rounded-xl" />
            </div>
          </div>
        </div>
      </div>
    );
  }

  if (error || !preset) {
    return (
      <DetailLoadError
        title={<Trans>Error loading preset</Trans>}
        error={error}
        notFound={<Trans>Preset not found</Trans>}
        back={{
          to: '/pipeline/presets',
          label: <Trans>Back to presets</Trans>,
        }}
      />
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
