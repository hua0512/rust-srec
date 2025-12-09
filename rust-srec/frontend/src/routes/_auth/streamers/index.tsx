import { createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { streamerApi, configApi } from '../../../api/endpoints';
import { Button } from '../../../components/ui/button';
import { useState } from 'react';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { StreamerSchema } from '../../../api/schemas';
import { z } from 'zod';
import { StreamersToolbar } from '../../../components/streamers/streamers-toolbar';
import { StreamerCard } from '../../../components/streamers/streamer-card';
import { Loader2, ChevronLeft, ChevronRight } from 'lucide-react';

export const Route = createFileRoute('/_auth/streamers/')({
  component: StreamersPage,
});

function StreamersPage() {
  const [page, setPage] = useState(1);
  const [search, setSearch] = useState('');
  const [platformFilter, setPlatformFilter] = useState('all');
  const [stateFilter, setStateFilter] = useState('all');

  const queryClient = useQueryClient();
  const limit = 20; // Items per page

  // Fetch Platforms for filter
  const { data: platforms = [] } = useQuery({
    queryKey: ['platforms'],
    queryFn: () => configApi.listPlatforms(),
  });

  // Fetch Streamers
  const { data: streamersData, isLoading, isFetching } = useQuery({
    queryKey: ['streamers', page, search, platformFilter, stateFilter],
    queryFn: async () => {
      // Backend expects 'platform' and 'state' params.
      const platform = platformFilter === 'all' ? undefined : platformFilter;
      const state = stateFilter === 'all' ? undefined : stateFilter;
      // API returns { items: [], total: number, ... }
      return streamerApi.list({ page, limit, search, platform, state });
    },
    placeholderData: (previousData) => previousData,
    refetchInterval: 5000,
  });

  const streamers = streamersData?.items || [];
  const totalStreamers = streamersData?.total || 0;
  const totalPages = Math.ceil(totalStreamers / limit);

  const deleteMutation = useMutation({
    mutationFn: streamerApi.delete,
    onSuccess: () => {
      toast.success(t`Streamer deleted`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to delete streamer`);
    },
  });

  const checkMutation = useMutation({
    mutationFn: streamerApi.check,
    onSuccess: () => {
      toast.success(t`Check triggered`);
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to trigger check`);
    },
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      streamerApi.update(id, { enabled }),
    onSuccess: () => {
      toast.success(t`Streamer updated`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update streamer`);
    },
  });

  const handleDelete = (id: string) => {
    if (confirm(t`Are you sure you want to delete this streamer?`)) {
      deleteMutation.mutate(id);
    }
  };

  const handleResetFilters = () => {
    setSearch('');
    setPlatformFilter('all');
    setStateFilter('all');
    setPage(1);
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-3xl font-bold tracking-tight"><Trans>Streamers</Trans></h1>
      </div>

      <StreamersToolbar
        search={search}
        onSearchChange={(v) => { setSearch(v); setPage(1); }}
        platformFilter={platformFilter}
        onPlatformFilterChange={(v) => { setPlatformFilter(v); setPage(1); }}
        stateFilter={stateFilter}
        onStateFilterChange={(v) => { setStateFilter(v); setPage(1); }}
        platforms={platforms}
        onResetFilters={handleResetFilters}
      />

      {isLoading ? (
        <div className="flex justify-center items-center h-64">
          <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
        </div>
      ) : (
        <>
          {/* Cards Grid */}
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
            {streamers.map((streamer: z.infer<typeof StreamerSchema>) => (
              <StreamerCard
                key={streamer.id}
                streamer={streamer}
                onDelete={handleDelete}
                onToggle={(id, enabled) => toggleMutation.mutate({ id, enabled })}
                onCheck={(id) => checkMutation.mutate(id)}
              />
            ))}
          </div>

          {/* Empty State */}
          {streamers.length === 0 && (
            <div className="text-center py-12 text-muted-foreground bg-muted/20 rounded-lg border border-dashed">
              <p><Trans>No streamers found.</Trans></p>
            </div>
          )}

          {/* Pagination Controls */}
          <div className="flex items-center justify-end space-x-2 py-4">
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPage((p) => Math.max(1, p - 1))}
              disabled={page === 1 || isFetching}
            >
              <ChevronLeft className="h-4 w-4 mr-1" />
              <Trans>Previous</Trans>
            </Button>
            <div className="text-sm font-medium">
              <Trans>Page {page} of {Math.max(1, totalPages)}</Trans>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPage((p) => p + 1)}
              disabled={page >= totalPages || isFetching}
            >
              <Trans>Next</Trans>
              <ChevronRight className="h-4 w-4 ml-1" />
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
