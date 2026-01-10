import {
  getSystemHealth,
  getPipelineStats,
  listStreamers,
  deleteStreamer,
  checkStreamer,
  updateStreamer,
} from '@/server/functions';
import { CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { DashboardCard } from '@/components/dashboard/dashboard-card';
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
import { useLingui } from '@lingui/react';
import { t } from '@lingui/core/macro';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useMutation, useQueryClient, useQuery } from '@tanstack/react-query';
import { toast } from 'sonner';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { formatRelativeTime } from '@/lib/date-utils';
import { formatDuration } from '@/lib/format';

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
    <div className="min-h-screen p-3 md:p-8 space-y-6 md:space-y-8 bg-gradient-to-br from-background via-background to-muted/20">
      <motion.div
        variants={container}
        initial="hidden"
        animate="show"
        className="space-y-10 relative z-10"
      >
        {/* System Health Section */}
        <section className="space-y-6">
          <motion.div
            variants={item}
            className="flex items-center justify-between"
          >
            <h2 className="text-xl font-bold tracking-tight text-foreground flex items-center gap-3">
              <div className="p-2 rounded-lg bg-primary/10 text-primary">
                <Activity className="h-5 w-5" />
              </div>
              <Trans>System Status</Trans>
            </h2>
            <Button
              variant="outline"
              size="sm"
              asChild
              className="bg-card/50 backdrop-blur-md border-primary/20 hover:bg-primary/10 hover:border-primary/40 transition-all duration-300"
            >
              <Link to="/system/health">
                <Trans>View Details</Trans>
              </Link>
            </Button>
          </motion.div>

          <div className="grid gap-4 md:gap-6 md:grid-cols-2 lg:grid-cols-4">
            {isHealthLoading || !health ? (
              Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="h-32 rounded-2xl bg-muted/20" />
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
                  <DashboardCard className="h-full">
                    <CardHeader className="pb-2">
                      <CardTitle className="text-sm font-medium text-muted-foreground uppercase tracking-wider">
                        <Trans>Overall Health</Trans>
                      </CardTitle>
                    </CardHeader>
                    <CardContent>
                      <div className="flex items-center gap-4">
                        <div className="relative">
                          <div
                            className={cn(
                              'h-4 w-4 rounded-full',
                              health.status === 'healthy'
                                ? 'bg-green-500'
                                : health.status === 'degraded'
                                  ? 'bg-yellow-500'
                                  : 'bg-red-500',
                            )}
                          />
                          <div
                            className={cn(
                              'absolute inset-0 rounded-full animate-ping opacity-75',
                              health.status === 'healthy'
                                ? 'bg-green-500'
                                : health.status === 'degraded'
                                  ? 'bg-yellow-500'
                                  : 'bg-red-500',
                            )}
                          />
                        </div>
                        <span className="text-2xl font-bold capitalize bg-clip-text text-transparent bg-gradient-to-r from-foreground to-foreground/70">
                          {getStatusLabel(health.status)}
                        </span>
                      </div>
                      <p className="text-xs font-medium text-muted-foreground/70 mt-3 flex items-center gap-2">
                        <Activity className="w-3 h-3" />
                        <span className="font-mono">
                          {formatDuration(health.uptime_secs)}
                        </span>{' '}
                        uptime
                      </p>
                    </CardContent>
                  </DashboardCard>
                </motion.div>

                {/* Key Components */}
                <ComponentStatusCard
                  name={t`Database`}
                  component={health.components.find(
                    (c: any) => c.name === 'database',
                  )}
                  icon={HardDrive}
                />
                <ComponentStatusCard
                  name={t`Download Manager`}
                  component={health.components.find(
                    (c: any) => c.name === 'download_manager',
                  )}
                  icon={Activity}
                />
                <ComponentStatusCard
                  name={t`Disk`}
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
        <section className="space-y-6">
          <motion.div variants={item}>
            <h2 className="text-xl font-bold tracking-tight text-foreground flex items-center gap-3">
              <div className="p-2 rounded-lg bg-primary/10 text-primary">
                <Activity className="h-5 w-5" />
              </div>
              <Trans>Pipeline Statistics</Trans>
            </h2>
          </motion.div>
          <div className="grid gap-4 md:gap-6 md:grid-cols-2 lg:grid-cols-4">
            <StatCard
              title={<Trans>Pending Jobs</Trans>}
              icon={Circle}
              value={stats?.pending_count}
              loading={isStatsLoading}
              color="text-yellow-500"
              bg="bg-yellow-500/10"
              href="/pipeline/jobs"
              search={{ status: 'PENDING' }}
            />
            <StatCard
              title={<Trans>Processing</Trans>}
              icon={Activity}
              value={stats?.processing_count}
              loading={isStatsLoading}
              color="text-blue-500"
              bg="bg-blue-500/10"
              href="/pipeline/jobs"
              search={{ status: 'PROCESSING' }}
            />
            <StatCard
              title={<Trans>Completed</Trans>}
              icon={CheckCircle}
              value={stats?.completed_count}
              loading={isStatsLoading}
              color="text-green-500"
              bg="bg-green-500/10"
              href="/pipeline/jobs"
              search={{ status: 'COMPLETED' }}
            />
            <StatCard
              title={<Trans>Failed</Trans>}
              icon={XCircle}
              value={stats?.failed_count}
              loading={isStatsLoading}
              color="text-red-500"
              bg="bg-red-500/10"
              href="/pipeline/jobs"
              search={{ status: 'FAILED' }}
            />
          </div>
        </section>

        {/* Active Recordings Section */}
        <section className="space-y-6">
          <motion.div
            variants={item}
            className="flex items-center justify-between"
          >
            <h2 className="text-xl font-bold tracking-tight text-foreground flex items-center gap-3">
              <div className="p-2 rounded-lg bg-red-500/10 text-red-500">
                <PlayCircle className="h-5 w-5" />
              </div>
              <Trans>Active Recordings</Trans>
            </h2>
            {activeStreamers.length > 0 && (
              <Button
                variant="outline"
                size="sm"
                asChild
                className="bg-card/50 backdrop-blur-md border-primary/20 hover:bg-primary/10 hover:border-primary/40 transition-all duration-300"
              >
                <Link to="/streamers">
                  <Trans>View All</Trans>
                </Link>
              </Button>
            )}
          </motion.div>

          <div className="grid gap-4 md:gap-6 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
            {isStreamersLoading ? (
              Array.from({ length: 4 }).map((_, i) => (
                <Skeleton
                  key={i}
                  className="h-[200px] w-full rounded-2xl bg-muted/20"
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
                className="col-span-full flex flex-col items-center justify-center p-12 text-center space-y-4 border border-dashed border-white/10 rounded-3xl bg-card/30 backdrop-blur-sm"
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
  const { i18n } = useLingui();

  if (!component)
    return (
      <motion.div
        variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
        className="h-full"
      >
        <DashboardCard className="h-full opacity-60">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground uppercase tracking-wider">
              {name}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-sm text-muted-foreground font-mono">
              <Trans>Not available</Trans>
            </div>
          </CardContent>
        </DashboardCard>
      </motion.div>
    );

  const isHealthy = component.status === 'healthy';

  return (
    <motion.div
      variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
      className="h-full"
    >
      <DashboardCard
        className={cn('h-full', !isHealthy && 'border-red-500/30 bg-red-500/5')}
      >
        <CardHeader className="pb-2 flex flex-row items-center justify-between space-y-0">
          <CardTitle className="text-sm font-medium text-muted-foreground uppercase tracking-wider">
            {name}
          </CardTitle>
          <div
            className={cn(
              'p-1.5 rounded-md transition-colors',
              isHealthy
                ? 'bg-secondary/50 text-secondary-foreground'
                : 'bg-red-500/10 text-red-500',
            )}
          >
            <Icon className="h-4 w-4" />
          </div>
        </CardHeader>
        <CardContent>
          <div className="flex items-center gap-3">
            <div className="relative flex h-3 w-3">
              {!isHealthy && (
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75"></span>
              )}
              <span
                className={cn(
                  'relative inline-flex rounded-full h-3 w-3',
                  isHealthy
                    ? 'bg-green-500'
                    : component.status === 'degraded'
                      ? 'bg-yellow-500'
                      : 'bg-red-500',
                )}
              ></span>
            </div>

            <div
              className={cn(
                'text-lg font-bold capitalize truncate tracking-tight',
                !isHealthy && 'text-red-500 dark:text-red-400',
              )}
            >
              {!isHealthy && component.message && component.message.length < 30
                ? component.message
                : getStatusLabel(component.status)}
            </div>
          </div>
          {component.last_check && (
            <p className="text-[10px] text-muted-foreground/60 mt-3 font-mono">
              <Trans>Updated</Trans>{' '}
              {formatRelativeTime(component.last_check, i18n.locale)}
            </p>
          )}
        </CardContent>
      </DashboardCard>
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
  href,
  search,
}: {
  title: React.ReactNode;
  icon: any;
  value?: number;
  loading: boolean;
  color?: string;
  bg?: string;
  href?: string;
  search?: any;
}) {
  const content = (
    <DashboardCard
      className={cn(
        'h-full transition-all duration-300',
        href &&
          'hover:border-primary/40 hover:shadow-lg hover:shadow-primary/5 cursor-pointer active:scale-[0.98]',
      )}
    >
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground uppercase tracking-wider">
          {title}
        </CardTitle>
        <div
          className={cn(
            'p-2 rounded-xl transition-colors ring-1 ring-inset ring-black/5',
            bg || 'bg-muted',
          )}
        >
          <Icon className={cn('h-4 w-4', color || 'text-muted-foreground')} />
        </div>
      </CardHeader>
      <CardContent className="relative z-10">
        {loading ? (
          <Skeleton className="h-9 w-16 rounded-lg bg-muted/20" />
        ) : (
          <div className="text-2xl font-bold tracking-tighter text-foreground">
            {value}
          </div>
        )}
      </CardContent>
    </DashboardCard>
  );

  if (href) {
    return (
      <motion.div
        variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
      >
        <Link to={href} search={search}>
          {content}
        </Link>
      </motion.div>
    );
  }

  return (
    <motion.div
      variants={{ hidden: { opacity: 0, y: 20 }, show: { opacity: 1, y: 0 } }}
    >
      {content}
    </motion.div>
  );
}

function getStatusLabel(status: string) {
  switch (status.toLowerCase()) {
    case 'healthy':
      return t`Healthy`;
    case 'degraded':
      return t`Degraded`;
    case 'unhealthy':
      return t`Unhealthy`;
    case 'unknown':
      return t`Unknown`;
    default:
      return status;
  }
}
