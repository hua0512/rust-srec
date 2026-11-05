import { useCallback } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { deleteStreamer, updateStreamer } from '@/server/functions';

/**
 * The delete and enable/disable actions of a streamer card. The handlers are
 * stable across renders, so memoised cards keep their props while the list
 * polls. The card's menu asks for confirmation before calling `handleDelete`.
 */
export function useStreamerMutations() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteStreamer({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Streamer deleted`));
      void queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error) =>
      toast.error(error.message || i18n._(msg`Failed to delete streamer`)),
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      updateStreamer({ data: { id, data: { enabled } } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Streamer updated`));
      void queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error) =>
      toast.error(error.message || i18n._(msg`Failed to update streamer`)),
  });

  // `mutate` is stable while the mutation result object is not.
  const deleteMutate = deleteMutation.mutate;
  const toggleMutate = toggleMutation.mutate;

  const handleDelete = useCallback(
    (id: string) => deleteMutate(id),
    [deleteMutate],
  );
  const handleToggle = useCallback(
    (id: string, enabled: boolean) => toggleMutate({ id, enabled }),
    [toggleMutate],
  );

  return { handleDelete, handleToggle };
}
