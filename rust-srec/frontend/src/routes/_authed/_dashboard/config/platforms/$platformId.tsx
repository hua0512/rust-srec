import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { getPlatformConfig, updatePlatformConfig } from '@/server/functions';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import {
  PlatformEditor,
  EditPlatformFormValues,
} from '@/components/config/platforms/platform-editor';
import { Skeleton } from '@/components/ui/skeleton';

export const Route = createFileRoute(
  '/_authed/_dashboard/config/platforms/$platformId',
)({
  component: EditPlatformPage,
});

function EditPlatformPage() {
  const { platformId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const {
    data: platform,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['config', 'platform', platformId],
    queryFn: () => getPlatformConfig({ data: platformId }),
  });

  const updateMutation = useMutation({
    mutationFn: (data: EditPlatformFormValues) =>
      updatePlatformConfig({
        data: {
          id: platformId,
          data: { ...data, id: platformId, name: platform!.name },
        },
      }),
    onSuccess: () => {
      toast.success(i18n._(msg`Platform configuration updated successfully`));
      queryClient.invalidateQueries({ queryKey: ['config', 'platforms'] });
      queryClient.invalidateQueries({
        queryKey: ['config', 'platform', platformId],
      });
      navigate({ to: '/config/platforms' });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to update platform: ${error.message}`)),
  });

  const onSubmit = (data: EditPlatformFormValues) => {
    console.log('updatePlatformConfig input:', data);
    updateMutation.mutate(data);
  };

  if (isLoading) {
    return (
      <div className="space-y-6">
        <div className="flex gap-2">
          {[1, 2, 3, 4, 5].map((i) => (
            <Skeleton key={i} className="h-10 w-32 rounded-lg" />
          ))}
        </div>
        <div className="space-y-4">
          <Skeleton className="h-24 w-full rounded-xl" />
          <Skeleton className="h-48 w-full rounded-xl" />
          <Skeleton className="h-48 w-full rounded-xl" />
        </div>
      </div>
    );
  }

  if (error || !platform) {
    return (
      <div className="flex flex-col items-center justify-center p-20 text-center">
        <h3 className="text-xl font-bold text-destructive">
          <Trans>Error loading platform</Trans>
        </h3>
        <p className="text-muted-foreground mt-2">
          {error?.message || i18n._(msg`Platform not found`)}
        </p>
      </div>
    );
  }

  return (
    <PlatformEditor
      platform={platform}
      onSubmit={onSubmit}
      isUpdating={updateMutation.isPending}
    />
  );
}
