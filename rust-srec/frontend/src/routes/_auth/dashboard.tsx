import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { systemApi, pipelineApi, streamerApi } from '../../api/endpoints';
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/card';
import { Skeleton } from '../../components/ui/skeleton';
import { Activity, Cpu, HardDrive, Clock, CheckCircle, XCircle, AlertCircle, PlayCircle, Circle } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { getPlatformFromUrl } from '../../lib/utils';

export const Route = createFileRoute('/_auth/dashboard')({
  component: Dashboard,
});

function Dashboard() {
  const { data: health, isLoading: isHealthLoading } = useQuery({
    queryKey: ['health'],
    queryFn: systemApi.getHealth,
    refetchInterval: 5000,
  });

  const { data: stats, isLoading: isStatsLoading } = useQuery({
    queryKey: ['pipeline-stats'],
    queryFn: pipelineApi.getStats,
    refetchInterval: 5000,
  });

  const { data: streamers, isLoading: isStreamersLoading } = useQuery({
    queryKey: ['streamers', 'active'],
    queryFn: () => streamerApi.list({ limit: 100, state: 'LIVE' }),
    refetchInterval: 5000,
  });

  const activeStreamers = streamers?.items || [];

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold tracking-tight"><Trans>Dashboard</Trans></h1>

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
            <Card key={streamer.id}>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">
                  {streamer.name}
                </CardTitle>
                <PlayCircle className="h-4 w-4 text-red-500 animate-pulse" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{getPlatformFromUrl(streamer.url)}</div>
                <p className="text-xs text-muted-foreground truncate">
                  {streamer.url}
                </p>
                <div className="mt-2 text-xs font-medium px-2 py-1 rounded-full bg-red-100 text-red-800 dark:bg-red-900 dark:text-red-200 inline-block">
                  {streamer.state}
                </div>
              </CardContent>
            </Card>
          ))
        ) : (
          <div className="col-span-full flex flex-col items-center justify-center p-8 text-muted-foreground border border-dashed rounded-lg">
            <AlertCircle className="h-8 w-8 mb-2" />
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
