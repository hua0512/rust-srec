import {
  getSystemHealth,
  getPipelineStats,
  listStreamers,
  deleteStreamer,
  checkStreamer,
  updateStreamer,
} from '@/server/functions';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { StreamerCard } from '@/components/streamers/streamer-card';
import {
  Activity,
  HardDrive,
  CheckCircle,
  XCircle,
  Circle,
  PlayCircle,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { formatDistanceToNow } from 'date-fns';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useMutation, useQueryClient, useQuery } from '@tanstack/react-query';
import { toast } from 'sonner';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';

export const Route = createFileRoute('/_authed/_dashboard/dashboard')({
  component: Dashboard,
});

function Dashboard() {
  const queryClient = useQueryClient();

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
    onSuccess: () => toast.success(t`Check triggered`),
    onError: (error: any) =>
      toast.error(error.message || t`Failed to trigger check`),
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      updateStreamer({ data: { id, data: { enabled } } }),
    onSuccess: () => {
      toast.success(t`Streamer updated`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) =>
      toast.error(error.message || t`Failed to update streamer`),
  });

  const handleDelete = (id: string) => {
    if (confirm(t`Are you sure you want to delete this streamer?`)) {
      deleteMutation.mutate(id);
    }
  };

  const container = {
    hidden: { opacity: 0 },
    show: {
      opacity: 1,
      transition: {
        staggerChildren: 0.1,
      },
    },
  };

  const item = {
    hidden: { opacity: 0, y: 20 },
    show: { opacity: 1, y: 0 },
  };

  return (
    <div className="min-h-screen p-4 md:p-8 space-y-8">
      <motion.div
        variants={container}
        initial="hidden"
        animate="show"
        className="space-y-10"
      >
        {/* System Health Section */}
        <section className="space-y-4">
          <motion.div
            variants={item}
            className="flex items-center justify-between"
          >
            <h2 className="text-lg font-semibold tracking-tight text-foreground/90 flex items-center gap-2">
              <Activity className="h-4 w-4 text-primary" />
              <Trans>System Status</Trans>
            </h2>
            <Button
              variant="ghost"
              size="sm"
              asChild
              className="text-muted-foreground hover:text-primary"
            >
              <Link to="/system/health">
                <Trans>View Details</Trans>
              </Link>
            </Button>
          </motion.div>

          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
            {isHealthLoading || !health ? (
              Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="h-28 rounded-xl" />
              ))
            ) : (
              <motion.div
                variants={container}
                initial="hidden"
                animate="show"
                className="contents"
              >
                {/* Summary Card */}
                <motion.div variants={item} className="h-full">
                  <Card className="bg-card/50 backdrop-blur-sm border-primary/5 shadow-sm h-full">
                    <CardHeader className="pb-2">
                      <CardTitle className="text-sm font-medium text-muted-foreground">
                        <Trans>Overall Health</Trans>
                      </CardTitle>
                    </CardHeader>
                    <CardContent>
                      <div className="flex items-center gap-3">
                        <div
                          className={cn(
                            'h-3 w-3 rounded-full',
                            health.status === 'healthy'
                              ? 'bg-green-500 shadow-[0_0_8px] shadow-green-500/50'
                              : health.status === 'degraded'
                                ? 'bg-yellow-500 shadow-[0_0_8px] shadow-yellow-500/50'
                                : 'bg-red-500',
                          )}
                        />
                        <span className="text-2xl font-bold capitalize">
                          {health.status}
                        </span>
                      </div>
                      <p className="text-xs text-muted-foreground mt-2">
                        <Trans>Uptime</Trans>:{' '}
                        {formatUptime(health.uptime_secs)}
                      </p>
                    </CardContent>
                  </Card>
                </motion.div>

                {/* Key Components */}
                <ComponentStatusCard
                  name="Database"
                  component={health.components.find(
                    (c: any) => c.name === 'database',
                  )}
                  icon={HardDrive}
                />
                <ComponentStatusCard
                  name="Download Manager"
                  component={health.components.find(
                    (c: any) => c.name === 'download_manager',
                  )}
                  icon={Activity}
                />
                <ComponentStatusCard
                  name="Disk"
                  component={health.components.find((c: any) =>
                    c.name.startsWith('disk:'),
                  )}
                  icon={HardDrive}
                />
              </motion.div>
            )}
          </div>
        </section>

        {/* Pipeline Stats Section */}
        <section className="space-y-4">
          <motion.div variants={item}>
            <h2 className="text-lg font-semibold tracking-tight text-foreground/90 flex items-center gap-2">
              <Activity className="h-4 w-4 text-primary" />
              <Trans>Pipeline Statistics</Trans>
            </h2>
          </motion.div>
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
            <StatCard
              title={<Trans>Pending Jobs</Trans>}
              icon={Circle}
              value={stats?.pending_count}
              loading={isStatsLoading}
              color="text-yellow-600 dark:text-yellow-400"
              bg="bg-yellow-500/10"
            />
            <StatCard
              title={<Trans>Processing</Trans>}
              icon={Activity}
              value={stats?.processing_count}
              loading={isStatsLoading}
              color="text-blue-600 dark:text-blue-400"
              bg="bg-blue-500/10"
            />
            <StatCard
              title={<Trans>Completed</Trans>}
              icon={CheckCircle}
              value={stats?.completed_count}
              loading={isStatsLoading}
              color="text-green-600 dark:text-green-400"
              bg="bg-green-500/10"
            />
            <StatCard
              title={<Trans>Failed</Trans>}
              icon={XCircle}
              value={stats?.failed_count}
              loading={isStatsLoading}
              color="text-red-600 dark:text-red-400"
              bg="bg-red-500/10"
            />
          </div>
        </section>

        {/* Active Recordings Section */}
        <section className="space-y-4">
          <motion.div
            variants={item}
            className="flex items-center justify-between"
          >
            <h2 className="text-lg font-semibold tracking-tight text-foreground/90 flex items-center gap-2">
              <PlayCircle className="h-4 w-4 text-red-500" />
              <Trans>Active Recordings</Trans>
            </h2>
            {activeStreamers.length > 0 && (
              <Button
                variant="ghost"
                size="sm"
                asChild
                className="text-muted-foreground hover:text-primary"
              >
                <Link to="/streamers">
                  <Trans>View All</Trans>
                </Link>
              </Button>
            )}
          </motion.div>

          <div className="grid gap-6 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
            {isStreamersLoading ? (
              Array.from({ length: 4 }).map((_, i) => (
                <Skeleton
                  key={i}
                  className="h-[200px] w-full rounded-xl bg-muted/10"
                />
              ))
            ) : activeStreamers.length > 0 ? (
              activeStreamers.map((streamer, index) => (
                <motion.div
                  key={streamer.id}
                  initial={{ opacity: 0, y: 20 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ delay: 0.4 + index * 0.05 }}
                >
                  <StreamerCard
                    streamer={streamer}
                    onDelete={handleDelete}
                    onToggle={(id, enabled) =>
                      toggleMutation.mutate({ id, enabled })
                    }
                    onCheck={(id) => checkMutation.mutate(id)}
                  />
                </motion.div>
              ))
            ) : (
              <motion.div
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: 0.4 }}
                className="col-span-full flex flex-col items-center justify-center p-12 text-center space-y-4 border-2 border-dashed border-muted-foreground/10 rounded-xl bg-muted/5"
              >
                <div className="p-4 rounded-full bg-muted/20">
                  <Activity className="h-8 w-8 text-muted-foreground/50" />
                </div>
                <div className="space-y-1">
                  <h3 className="font-medium text-muted-foreground">
                    <Trans>No active recordings</Trans>
                  </h3>
                  <p className="text-sm text-muted-foreground/60">
                    <Trans>Streamers currently live will appear here.</Trans>
                  </p>
                </div>
              </motion.div>
            )}
          </div>
        </section>
      </motion.div>
    </div>
  );
}

function ComponentStatusCard({
  name,
  component,
  icon: Icon,
}: {
  name: string;
  component: any;
  icon: any;
}) {
  if (!component)
    return (
      <motion.div
        variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
        className="h-full"
      >
        <Card className="bg-card/50 backdrop-blur-sm border-primary/5 shadow-sm opacity-50 h-full">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              {name}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-sm text-muted-foreground">
              <Trans>Not available</Trans>
            </div>
          </CardContent>
        </Card>
      </motion.div>
    );

  const isHealthy = component.status === 'healthy';

  return (
    <motion.div
      variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
      className="h-full"
    >
      <Card
        className={cn(
          'bg-card/50 backdrop-blur-sm border-primary/5 shadow-sm transition-all hover:shadow-md h-full',
          !isHealthy &&
          'border-red-500/20 bg-red-500/5 text-red-600 dark:text-red-400',
        )}
      >
        <CardHeader className="pb-2 flex flex-row items-center justify-between space-y-0">
          <CardTitle className="text-sm font-medium text-muted-foreground">
            {name}
          </CardTitle>
          <Icon
            className={cn(
              'h-4 w-4',
              isHealthy ? 'text-muted-foreground' : 'text-red-500',
            )}
          />
        </CardHeader>
        <CardContent>
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'h-3 w-3 rounded-full shrink-0',
                isHealthy
                  ? 'bg-green-500 shadow-[0_0_8px] shadow-green-500/50'
                  : component.status === 'degraded'
                    ? 'bg-yellow-500 shadow-[0_0_8px] shadow-yellow-500/50'
                    : 'bg-red-500',
              )}
            />
            <div
              className={cn(
                'text-2xl font-bold capitalize truncate',
                !isHealthy && 'text-red-600 dark:text-red-400',
              )}
            >
              {/* For key components on dashboard, prefer simple status unless it's an error message that fits */}
              {!isHealthy && component.message && component.message.length < 30
                ? component.message
                : component.status}
            </div>
          </div>
          {component.last_check && (
            <p className="text-[10px] text-muted-foreground mt-2 text-right">
              <Trans>Checked</Trans>{' '}
              {formatDistanceToNow(new Date(component.last_check), {
                addSuffix: true,
              })}
            </p>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}

function StatCard({
  title,
  icon: Icon,
  value,
  loading,
  color,
  bg,
}: {
  title: React.ReactNode;
  icon: any;
  value?: number;
  loading: boolean;
  color?: string;
  bg?: string;
}) {
  return (
    <motion.div
      variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
    >
      <Card className="overflow-hidden bg-card/50 backdrop-blur-sm border-primary/5 hover:border-primary/20 transition-all duration-300 shadow-sm hover:shadow-md group">
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium text-muted-foreground group-hover:text-foreground transition-colors">
            {title}
          </CardTitle>
          <div
            className={cn('p-2 rounded-lg transition-colors', bg || 'bg-muted')}
          >
            <Icon className={cn('h-4 w-4', color || 'text-muted-foreground')} />
          </div>
        </CardHeader>
        <CardContent>
          {loading ? (
            <Skeleton className="h-8 w-10" />
          ) : (
            <div className="text-2xl font-bold tracking-tight text-foreground/90">
              {value}
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}
