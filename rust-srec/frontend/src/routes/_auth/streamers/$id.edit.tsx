import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { streamerApi } from '../../../api/endpoints';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { Skeleton } from '../../../components/ui/skeleton';

import { StreamerForm } from '../../../components/streamers/streamer-form';
import { CreateStreamerSchema, UpdateStreamerSchema } from '../../../api/schemas';
import { z } from 'zod';
import { useDownloadProgress } from '../../../hooks/useDownloadProgress';

export const Route = createFileRoute('/_auth/streamers/$id/edit')({
  component: EditStreamerPage,
});

function EditStreamerPage() {
  const { id } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // Subscribe to download progress updates for this specific streamer
  // Requirements: 6.1, 6.2, 6.3 - Filter updates by streamer when viewing single streamer page
  // Sends subscribe message when entering page, unsubscribe when leaving (via cleanup)
  useDownloadProgress({ streamerId: id });

  const { data: streamer, isLoading: isStreamerLoading } = useQuery({
    queryKey: ['streamer', id],
    queryFn: () => streamerApi.get(id),
  });

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof UpdateStreamerSchema>) => streamerApi.update(id, data),
    onSuccess: () => {
      toast.success(t`Streamer updated successfully`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
      queryClient.invalidateQueries({ queryKey: ['streamer', id] });
      navigate({ to: '/streamers' });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update streamer`);
    },
  });

  const onSubmit = (data: z.infer<typeof CreateStreamerSchema>) => {
    // Normalize "none" values to undefined
    const payload = {
      ...data,
      platform_config_id: data.platform_config_id === "none" ? undefined : data.platform_config_id,
      template_id: data.template_id === "none" ? undefined : data.template_id,
    };
    // UpdateStreamerSchema is partial, but form returns full object (CreateStreamerSchema structure).
    // API update usually accepts partial, but full object is also partial (superset).
    // Ensure types match if necessary, but here payload is compatible.
    updateMutation.mutate(payload);
  };

  if (isStreamerLoading) {
    return (
      <div className="max-w-2xl mx-auto space-y-6">
        <Skeleton className="h-10 w-48 mb-4" />
        <div className="border border-muted/40 shadow-sm rounded-lg p-6 space-y-6">
          <Skeleton className="h-8 w-1/3 mb-6" />
          <div className="space-y-4">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
          <Skeleton className="h-20 w-full mt-6" />
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <StreamerForm
        defaultValues={{
          name: streamer?.name,
          url: streamer?.url,
          priority: streamer?.priority,
          enabled: streamer?.enabled,
          platform_config_id: streamer?.platform_config_id || undefined,
          template_id: streamer?.template_id || undefined,
        }}
        onSubmit={onSubmit}
        isSubmitting={updateMutation.isPending}
        title={<Trans>Edit Streamer</Trans>}
        description={<Trans>Update configuration for {streamer?.name}.</Trans>}
        submitLabel={<Trans>Update Streamer</Trans>}
      />
    </div>
  );
}
