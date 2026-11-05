import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { queryKeys } from '@/api/query-keys';
import { createStreamer } from '@/server/functions';
import { StreamerEditor } from '@/components/streamers/streamer-editor';
import type { StreamerPayload } from '@/hooks/use-streamer-form';

export const Route = createLazyFileRoute('/_authed/_dashboard/streamers/new')({
  component: NewStreamerPage,
});

function NewStreamerPage() {
  const { i18n } = useLingui();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: (data: StreamerPayload) => createStreamer({ data }),
    onSuccess: () => {
      toast.success(i18n._(msg`Streamer created successfully`));
      void queryClient.invalidateQueries({ queryKey: queryKeys.streamers.all });
      void navigate({ to: '/streamers' });
    },
    onError: (error) => {
      toast.error(error.message || i18n._(msg`Failed to create streamer`));
    },
  });

  return (
    <StreamerEditor
      mode="create"
      onSubmit={(payload) => createMutation.mutate(payload)}
      isSubmitting={createMutation.isPending}
    />
  );
}
