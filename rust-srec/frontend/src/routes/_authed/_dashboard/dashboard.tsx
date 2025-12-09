import { getSystemHealth, getPipelineStats, listStreamers, deleteStreamer, checkStreamer, updateStreamer } from '@/server/functions';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { StreamerCard } from '@/components/streamers/streamer-card';
import { Activity, Cpu, HardDrive, Clock, CheckCircle, XCircle, AlertCircle, Circle } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQueryClient, useQuery } from '@tanstack/react-query';
import { toast } from 'sonner';

export const Route = createFileRoute('/_authed/_dashboard/dashboard')({
  component: Dashboard,
});

function Dashboard() {

  const { data: health, isLoading: isHealthLoading } = useQuery({
    queryKey: ['health'],
    queryFn: () => getSystemHealth(),
    refetchInterval: 5000,
  });

  const { data: stats, isLoading: isStatsLoading } = useQuery({
    queryKey: ['pipeline-stats'],
    queryFn: () => getPipelineStats(),
    refetchInterval: 5000,
  });

  const queryClient = useQueryClient();

  const { data: streamers, isLoading: isStreamersLoading } = useQuery({
    queryKey: ['streamers', 'active'],
    queryFn: () => listStreamers({ data: { limit: 100, state: 'LIVE' } }),
    refetchInterval: 5000,
  });

  const activeStreamers = streamers?.items || [];

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteStreamer({ data: id }),
    onSuccess: () => {
      toast.success(t`Streamer deleted`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to delete streamer`);
    },
  });

  const checkMutation = useMutation({
    mutationFn: (id: string) => checkStreamer({ data: id }),
    onSuccess: () => {
      toast.success(t`Check triggered`);
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to trigger check`);
    },
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      updateStreamer({ data: { id, data: { enabled } } }),
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

  return (
    <div className="space-y-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
      {/* System Health */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <HealthCard
          title={<Trans>CPU Usage</Trans>}
          icon={Cpu}
          value={health ? `${health.cpu_usage.toFixed(1)}%` : undefined}
          loading={isHealthLoading}
        />
        <HealthCard
          title={<Trans>Memory Usage</Trans>}
          icon={HardDrive}
          value={health ? `${health.memory_usage.toFixed(1)}%` : undefined}
          loading={isHealthLoading}
        />
        <HealthCard
          title={<Trans>Uptime</Trans>}
          icon={Clock}
          value={health ? formatUptime(health.uptime_secs) : undefined}
          loading={isHealthLoading}
        />
        <HealthCard
          title={<Trans>Version</Trans>}
          icon={Activity}
          value={health?.version}
          loading={isHealthLoading}
        />
      </div>

      {/* Pipeline Stats */}
      <h2 className="text-xl font-semibold tracking-tight"><Trans>Pipeline Statistics</Trans></h2>
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <StatCard
          title={<Trans>Pending Jobs</Trans>}
          icon={Circle}
          value={stats?.pending_count}
          loading={isStatsLoading}
          color="text-yellow-500"
        />
        <StatCard
          title={<Trans>Processing</Trans>}
          icon={Activity}
          value={stats?.processing_count}
          loading={isStatsLoading}
          color="text-blue-500"
        />
        <StatCard
          title={<Trans>Completed</Trans>}
          icon={CheckCircle}
          value={stats?.completed_count}
          loading={isStatsLoading}
          color="text-green-500"
        />
        <StatCard
          title={<Trans>Failed</Trans>}
          icon={XCircle}
          value={stats?.failed_count}
          loading={isStatsLoading}
          color="text-red-500"
        />
      </div>

      {/* Active Recordings */}
      <h2 className="text-xl font-semibold tracking-tight"><Trans>Active Recordings</Trans></h2>
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {isStreamersLoading ? (
          Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-32 w-full rounded-xl" />
          ))
        ) : activeStreamers.length > 0 ? (
          activeStreamers.map(streamer => (
            <StreamerCard
              key={streamer.id}
              streamer={streamer}
              onDelete={handleDelete}
              onToggle={(id, enabled) => toggleMutation.mutate({ id, enabled })}
              onCheck={(id) => checkMutation.mutate(id)}
            />
          ))
        ) : (
          <div className="col-span-full flex flex-col items-center justify-center p-8 text-muted-foreground border border-dashed rounded-lg bg-muted/20">
            <AlertCircle className="h-8 w-8 mb-2 opacity-50" />
            <p><Trans>No active recordings</Trans></p>
          </div>
        )}
      </div>
    </div>
  );
}

function HealthCard({ title, icon: Icon, value, loading }: { title: React.ReactNode, icon: any, value?: string | number, loading: boolean }) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium">
          {title}
        </CardTitle>
        <Icon className="h-4 w-4 text-muted-foreground" />
      </CardHeader>
      <CardContent>
        {loading ? (
          <Skeleton className="h-8 w-20" />
        ) : (
          <div className="text-2xl font-bold">{value}</div>
        )}
      </CardContent>
    </Card>
  )
}

function StatCard({ title, icon: Icon, value, loading, color }: { title: React.ReactNode, icon: any, value?: number, loading: boolean, color?: string }) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium">
          {title}
        </CardTitle>
        <Icon className={`h-4 w-4 ${color || 'text-muted-foreground'}`} />
      </CardHeader>
      <CardContent>
        {loading ? (
          <Skeleton className="h-8 w-10" />
        ) : (
          <div className="text-2xl font-bold">{value}</div>
        )}
      </CardContent>
    </Card>
  )
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}
