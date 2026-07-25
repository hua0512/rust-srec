import { lazy, Suspense, useMemo } from 'react';
import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { Filter as FilterIcon } from 'lucide-react';
import { useShallow } from 'zustand/react/shallow';

import { Skeleton } from '@/components/ui/skeleton';
import { useDownloadStore } from '@/store/downloads';
import { useDownloadProgress } from '@/hooks/use-download-progress';
import { getPlatformFromUrl } from '@/lib/utils';
import {
  deleteFilter,
  getStreamer,
  listFilters,
  listSessions,
  updateStreamer,
} from '@/server/functions';
import { StreamerEditor } from '@/components/streamers/streamer-editor';
import { StreamerHeader } from '@/components/streamers/edit/streamer-header';
import { ActiveDownloadCard } from '@/components/streamers/edit/active-download-card';
import { RecentSessionsList } from '@/components/streamers/edit/recent-sessions-list';
import { StatusCheckHistory } from '@/components/streamers/edit/status-check-history';
import { EditStreamerSkeleton } from '@/components/streamers/edit/edit-streamer-skeleton';
import type { StreamerPayload } from '@/hooks/use-streamer-form';

const StreamerFiltersTab = lazy(() =>
  import('@/components/streamers/edit/streamer-filters-tab').then((m) => ({
    default: m.StreamerFiltersTab,
  })),
);

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/streamers/$id/edit',
)({
  component: EditStreamerPage,
});

function EditStreamerPage() {
  const { id } = Route.useParams();
  const { i18n } = useLingui();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // Keeps the active-download card live while this page is open.
  useDownloadProgress({ streamerId: id });

  const { data: streamer, isLoading } = useQuery({
    queryKey: ['streamer', id],
    queryFn: () => getStreamer({ data: id }),
  });
  const { data: sessions, isLoading: isLoadingSessions } = useQuery({
    queryKey: ['sessions', { streamer_id: id }],
    queryFn: () => listSessions({ data: { streamer_id: id } }),
    enabled: !!id,
  });
  const { data: filters, isLoading: isFiltersLoading } = useQuery({
    queryKey: ['streamers', id, 'filters'],
    queryFn: () => listFilters({ data: id }),
    initialData: [],
  });

  const downloads = useDownloadStore(
    useShallow((state) => state.getDownloadsByStreamer(id)),
  );

  const updateMutation = useMutation({
    mutationFn: (data: StreamerPayload) =>
      updateStreamer({ data: { id, data } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Streamer updated successfully`));
      void queryClient.invalidateQueries({ queryKey: ['streamers'] });
      void queryClient.invalidateQueries({ queryKey: ['streamer', id] });
      void navigate({ to: '/streamers' });
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to update streamer`));
    },
  });

  const deleteFilterMutation = useMutation({
    mutationFn: (filterId: string) =>
      deleteFilter({ data: { streamerId: id, filterId } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Filter deleted successfully`));
      void queryClient.invalidateQueries({
        queryKey: ['streamers', id, 'filters'],
      });
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to delete filter`));
    },
  });

  const platform = useMemo(
    () => (streamer ? getPlatformFromUrl(streamer.url) : ''),
    [streamer],
  );

  if (isLoading || !streamer) {
    return <EditStreamerSkeleton />;
  }

  const isRecording = downloads.length > 0;

  return (
    <StreamerEditor
      mode="edit"
      streamer={streamer}
      onSubmit={(payload) => updateMutation.mutate(payload)}
      isSubmitting={updateMutation.isPending}
      header={
        <StreamerHeader
          streamer={streamer}
          isRecording={isRecording}
          isLive={streamer.state === 'LIVE'}
          platform={platform}
        />
      }
      sidebar={
        <>
          {isRecording && (
            <ActiveDownloadCard
              downloads={downloads}
              isRecording={isRecording}
            />
          )}
          <StatusCheckHistory streamerId={id} />
          <RecentSessionsList
            sessions={sessions?.items ?? []}
            isLoading={isLoadingSessions}
          />
        </>
      }
      extraTabs={[
        {
          value: 'filters',
          label: <Trans>Recording Filters</Trans>,
          icon: <FilterIcon className="h-4 w-4" />,
          count: filters?.length,
          content: (
            <Suspense fallback={<Skeleton className="h-64 w-full" />}>
              <StreamerFiltersTab
                streamerId={id}
                filters={filters ?? []}
                isLoading={isFiltersLoading}
                onDeleteFilter={(filterId) =>
                  deleteFilterMutation.mutate(filterId)
                }
              />
            </Suspense>
          ),
        },
      ]}
    />
  );
}
