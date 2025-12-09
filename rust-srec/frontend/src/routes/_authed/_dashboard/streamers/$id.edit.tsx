import { createFileRoute, useNavigate, Link } from '@tanstack/react-router';

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { Skeleton } from '../../../../components/ui/skeleton';

import { StreamerForm } from '../../../../components/streamers/streamer-form';
import { CreateStreamerSchema, UpdateStreamerSchema } from '../../../../api/schemas';
import { z } from 'zod';
import { useDownloadProgress } from '../../../../hooks/use-download-progress';
import { useDownloadStore } from '../../../../store/downloads';
import { useShallow } from 'zustand/react/shallow';

import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../../../components/ui/tabs';
import { Button } from '../../../../components/ui/button';
import { Plus, ArrowLeft, Settings, Filter as FilterIcon, Activity, FileVideo, Calendar, Clock, HardDrive } from 'lucide-react';
import { getStreamer, updateStreamer, listFilters, deleteFilter, listSessions } from '@/server/functions';
import { FilterList } from '../../../../components/filters/FilterList';
import { FilterDialog } from '../../../../components/filters/FilterDialog';
import { FilterSchema } from '../../../../api/schemas';
import { useState } from 'react';
import { Badge } from '../../../../components/ui/badge';
import { cn, getPlatformFromUrl } from '../../../../lib/utils';

export const Route = createFileRoute('/_authed/_dashboard/streamers/$id/edit')({
  component: EditStreamerPage,
});


function EditStreamerPage() {
  const { id } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // Subscribe to download progress updates for this specific streamer
  useDownloadProgress({ streamerId: id });

  const { data: streamer, isLoading: isStreamerLoading } = useQuery({
    queryKey: ['streamer', id],
    queryFn: () => getStreamer({ data: id }),
  });

  const { data: sessions, isLoading: isLoadingSessions } = useQuery({
    queryKey: ['sessions', { streamer_id: id }],
    queryFn: () => listSessions({ data: { streamer_id: id } }),
    enabled: !!id,
  });

  const recentSessions = sessions?.items ? [...sessions.items].sort((a: any, b: any) => new Date(b.start_time).getTime() - new Date(a.start_time).getTime()).slice(0, 5) : [];

  const downloads = useDownloadStore(useShallow(state => state.getDownloadsByStreamer(id)));
  const isRecording = downloads.length > 0;
  const isLive = streamer?.state === 'LIVE';
  const platform = streamer ? getPlatformFromUrl(streamer.url) : 'Unknown';

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof UpdateStreamerSchema>) => updateStreamer({ data: { id, data } }),
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
    const payload = {
      ...data,
      platform_config_id: data.platform_config_id === "none" ? undefined : data.platform_config_id,
      template_id: data.template_id === "none" ? undefined : data.template_id,
    };
    updateMutation.mutate(payload);
  };

  // Filters state
  const [filterDialogOpen, setFilterDialogOpen] = useState(false);
  const [filterToEdit, setFilterToEdit] = useState<z.infer<typeof FilterSchema> | null>(null);

  const { data: filters, isLoading: isFiltersLoading } = useQuery({
    queryKey: ['streamers', id, 'filters'],
    queryFn: () => listFilters({ data: id }),
  });

  const deleteFilterMutation = useMutation({
    mutationFn: (filterId: string) => deleteFilter({ data: { streamerId: id, filterId } }),
    onSuccess: () => {
      toast.success(t`Filter deleted successfully`);
      queryClient.invalidateQueries({ queryKey: ['streamers', id, 'filters'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to delete filter`);
    },
  });

  if (isStreamerLoading) {
    return (
      <div className="max-w-4xl mx-auto space-y-8 p-6 animate-pulse">
        <div className="flex items-center gap-4">
          <Skeleton className="h-10 w-10 rounded-full" />
          <div className="space-y-2">
            <Skeleton className="h-8 w-48" />
            <Skeleton className="h-4 w-24" />
          </div>
        </div>
        <div className="space-y-6">
          <Skeleton className="h-10 w-full rounded-xl" />
          <Skeleton className="h-[400px] w-full rounded-xl" />
        </div>
      </div>
    )
  }

  const statusColor = isRecording ? "text-red-500" : (isLive ? "text-green-500" : "text-muted-foreground");
  const statusBg = isRecording ? "bg-red-500/10 border-red-500/20" : (isLive ? "bg-green-500/10 border-green-500/20" : "bg-muted/50 border-transparent");

  return (
    <div className="max-w-5xl mx-auto p-4 md:p-8 space-y-8 animate-in fade-in slide-in-from-bottom-4 duration-500">
      {/* Header Section */}
      <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div className="flex items-center gap-4">
          <Button variant="ghost" size="icon" className="h-10 w-10 rounded-full bg-background border shadow-sm hover:bg-muted" asChild>
            <Link to="/streamers">
              <ArrowLeft className="h-5 w-5 text-muted-foreground" />
            </Link>
          </Button>

          <div className="space-y-1">
            <div className="flex items-center gap-3">
              <h1 className="text-3xl font-bold tracking-tight text-foreground">{streamer?.name}</h1>
              <Badge variant="outline" className={cn("capitalize px-2.5 py-0.5 text-xs font-semibold rounded-full border transition-colors", statusBg, statusColor)}>
                {isRecording ? <Trans>Recording</Trans> : (isLive ? <Trans>Live</Trans> : <Trans>Offline</Trans>)}
              </Badge>
            </div>
            <div className="flex items-center text-sm text-muted-foreground gap-2">
              <span className="capitalize">{platform.toLowerCase()}</span>
              <span>•</span>
              <span className="font-mono text-xs opacity-70">ID: {id}</span>
            </div>
          </div>
        </div>

        <div className="flex gap-2">
          {/* Potential Action Buttons could go here */}
        </div>
      </div>

      {/* Tabs Section */}
      <Tabs defaultValue="general" className="w-full">
        <TabsList className="w-full md:w-auto h-auto p-1 bg-muted/30 border rounded-full backdrop-blur-sm inline-flex">
          <TabsTrigger value="general" className="rounded-full px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all">
            <Settings className="w-4 h-4 mr-2" />
            <Trans>Configuration</Trans>
          </TabsTrigger>
          <TabsTrigger value="filters" className="rounded-full px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all">
            <FilterIcon className="w-4 h-4 mr-2" />
            <Trans>Recording Filters</Trans>
            {filters && filters.length > 0 && (
              <span className="ml-2 bg-primary/10 text-primary text-[10px] font-bold px-1.5 py-0.5 rounded-full">
                {filters.length}
              </span>
            )}
          </TabsTrigger>
        </TabsList>

        <TabsContent value="general" className="mt-8 animate-in fade-in slide-in-from-left-2 duration-300">
          <div className="md:grid md:grid-cols-3 gap-8">
            <div className="md:col-span-2">
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
                title={<Trans>General Settings</Trans>}
                description={<Trans>Manage core configuration for {streamer?.name}</Trans>}
                submitLabel={<Trans>Save Changes</Trans>}
              />
            </div>
            <div className="space-y-6 mt-6 md:mt-0">
              <div className="p-6 rounded-xl border bg-card/50 shadow-sm space-y-4">
                <div className="flex items-center gap-2 text-sm font-semibold text-muted-foreground">
                  <Activity className="w-4 h-4" /> <Trans>Status Info</Trans>
                </div>
                <div className="text-sm space-y-2">
                  <div className="flex justify-between">
                    <span className="text-muted-foreground"><Trans>Platform</Trans></span>
                    <span className="font-medium capitalize">{platform}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-muted-foreground"><Trans>Priority</Trans></span>
                    <span className="font-medium">{streamer?.priority}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-muted-foreground"><Trans>Auto-Record</Trans></span>
                    <span className={streamer?.enabled ? "text-emerald-500 font-medium" : "text-muted-foreground"}>
                      {streamer?.enabled ? <Trans>Enabled</Trans> : <Trans>Disabled</Trans>}
                    </span>
                  </div>
                </div>
              </div>

              {/* Recent Sessions Sidebar */}
              <div className="p-6 rounded-xl border bg-card/50 shadow-sm space-y-4">
                <div className="flex items-center justify-between text-sm font-semibold text-muted-foreground">
                  <div className="flex items-center gap-2">
                    <FileVideo className="w-4 h-4" /> <Trans>Recent Sessions</Trans>
                  </div>
                  <Link to="/sessions" search={{ streamer_id: id }} className="text-xs text-primary hover:underline"><Trans>View All</Trans></Link>
                </div>

                {isLoadingSessions ? (
                  <div className="space-y-3">
                    {[1, 2, 3].map(i => (
                      <div key={i} className="h-16 bg-muted/20 animate-pulse rounded-lg" />
                    ))}
                  </div>
                ) : recentSessions.length > 0 ? (
                  <div className="space-y-3">
                    {recentSessions.map(session => (
                      <div key={session.id} className="group flex flex-col gap-1 p-3 rounded-lg border bg-background/50 hover:bg-background hover:border-primary/20 transition-all">
                        <div className="flex items-center justify-between">
                          <span className="text-xs font-medium truncate max-w-[120px]" title={session.title}>{session.title}</span>
                          <span className={cn(
                            "text-[10px] px-1.5 py-0.5 rounded-full border",
                            session.end_time ? "bg-muted/30 text-muted-foreground border-transparent" : "bg-emerald-500/10 text-emerald-500 border-emerald-500/20"
                          )}>
                            {session.end_time ? 'Offline' : 'Live'}
                          </span>
                        </div>
                        <div className="flex items-center gap-3 text-[10px] text-muted-foreground">
                          <div className="flex items-center gap-1">
                            <Calendar className="w-3 h-3" />
                            <span>{new Date(session.start_time).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}</span>
                          </div>
                          <div className="flex items-center gap-1">
                            <Clock className="w-3 h-3" />
                            <span>{session.duration_secs ? `${Math.floor(session.duration_secs / 60)}m` : '-'}</span>
                          </div>
                          <div className="flex items-center gap-1 ml-auto">
                            <HardDrive className="w-3 h-3" />
                            <span>{session.total_size_bytes ? (session.total_size_bytes / 1024 / 1024).toFixed(1) + ' MB' : '-'}</span>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="text-sm text-muted-foreground py-4 text-center">
                    <Trans>No recent sessions found.</Trans>
                  </div>
                )}
              </div>
            </div>
          </div>
        </TabsContent>

        <TabsContent value="filters" className="mt-8 space-y-6 animate-in fade-in slide-in-from-right-2 duration-300">
          <div className="flex flex-col sm:flex-row justify-between items-start sm:items-center bg-gradient-to-r from-background to-muted/20 p-6 border rounded-2xl shadow-sm gap-4">
            <div className="space-y-1">
              <h3 className="text-xl font-bold tracking-tight"><Trans>Recording Filters</Trans></h3>
              <p className="text-sm text-muted-foreground max-w-lg">
                <Trans>Define precise rules for when `{streamer?.name}` should be recorded. Filters are additive (OR logic).</Trans>
              </p>
            </div>
            <Button size="lg" className="rounded-full shadow-lg hover:shadow-xl transition-all hover:scale-105" onClick={() => { setFilterToEdit(null); setFilterDialogOpen(true); }}>
              <Plus className="mr-2 h-5 w-5" />
              <Trans>Add New Filter</Trans>
            </Button>
          </div>

          {isFiltersLoading ? (
            <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
              <Skeleton className="h-40 w-full rounded-xl" /><Skeleton className="h-40 w-full rounded-xl" /><Skeleton className="h-40 w-full rounded-xl" />
            </div>
          ) : (
            <div className="min-h-[200px]">
              <FilterList
                filters={filters || []}
                onEdit={(filter) => { setFilterToEdit(filter); setFilterDialogOpen(true); }}
                onDelete={(filterId) => deleteFilterMutation.mutate(filterId)}
              />
            </div>
          )}
        </TabsContent>
      </Tabs>

      <FilterDialog
        streamerId={id}
        open={filterDialogOpen}
        onOpenChange={setFilterDialogOpen}
        filterToEdit={filterToEdit}
      />
    </div >
  );
}

