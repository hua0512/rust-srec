import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { getSystemHealth } from '@/server/functions';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import { HealthStatusBadge } from '@/components/health/health-status-badge';
import { Skeleton } from '@/components/ui/skeleton';
import { format, formatDistanceToNow } from 'date-fns';
import { useState, useEffect } from 'react';
import {
  Activity,
  Cpu,
  HardDrive,
  Clock,
  RefreshCw,
  AlertTriangle,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { type I18n } from '@lingui/core';
import { cn } from '@/lib/utils';
import { motion } from 'motion/react';

export const Route = createFileRoute('/_authed/_dashboard/system/health')({
  component: SystemHealthPage,
});

function SystemHealthPage() {
  const { i18n } = useLingui();

  // Client-side time to avoid hydration mismatch
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    setMounted(true);
  }, []);

  const {
    data: health,
    isLoading,
    refetch,
    isRefetching,
    error,
  } = useQuery({
    queryKey: ['health', 'detailed'],
    queryFn: () => getSystemHealth(),
    refetchInterval: 10000,
  });

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

  if (isLoading) {
    return (
      <div className="p-8 space-y-8">
        <div className="flex items-center justify-between">
          <Skeleton className="h-10 w-48" />
          <Skeleton className="h-10 w-32" />
        </div>
        <div className="grid gap-4 md:grid-cols-4">
          {[1, 2, 3, 4].map((i) => (
            <Skeleton key={i} className="h-32" />
          ))}
        </div>
        <Skeleton className="h-[400px]" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="p-8 flex flex-col items-center justify-center h-[50vh] space-y-4 text-center">
        <div className="p-4 rounded-full bg-red-500/10 text-red-500">
          <AlertTriangle className="h-8 w-8" />
        </div>
        <h2 className="text-xl font-semibold">
          <Trans>Failed to load system health</Trans>
        </h2>
        <p className="text-muted-foreground">
          <Trans>
            Could not connect to the server. Please check your connection.
          </Trans>
        </p>
        <Button onClick={() => refetch()} variant="outline">
          <Trans>Try Again</Trans>
        </Button>
      </div>
    );
  }

  if (!health) return null;

  return (
    <div className="p-4 md:p-8 space-y-8">
      <motion.div
        variants={container}
        initial="hidden"
        animate="show"
        className="space-y-8"
      >
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
          <div>
            <h1 className="text-3xl font-bold tracking-tight">
              <Trans>System Health</Trans>
            </h1>
            <p className="text-muted-foreground mt-1">
              <Trans>
                Real-time status of all system components and resources.
              </Trans>
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={() => refetch()}
            disabled={isRefetching}
            className={cn('gap-2', isRefetching && 'opacity-80')}
          >
            <RefreshCw
              className={cn('h-4 w-4', isRefetching && 'animate-spin')}
            />
            <Trans>Refresh</Trans>
          </Button>
        </div>

        {/* Top level metrics */}
        <motion.div
          variants={item}
          className="grid gap-4 md:grid-cols-2 lg:grid-cols-4"
        >
          <MetricCard
            title={<Trans>Status</Trans>}
            icon={Activity}
            content={
              <HealthStatusBadge
                status={health.status}
                className="text-base px-3 py-1"
              />
            }
            description={<Trans>Overall system status</Trans>}
            color="text-primary"
          />
          <MetricCard
            title={<Trans>Uptime</Trans>}
            icon={Clock}
            content={formatUptime(health.uptime_secs)}
            description={
              mounted ? (
                <Trans>
                  Since{' '}
                  {i18n.date(new Date(Date.now() - health.uptime_secs * 1000), {
                    timeStyle: 'medium',
                  })}
                </Trans>
              ) : (
                <span>-</span>
              )
            }
            color="text-green-500"
          />
          <MetricCard
            title={<Trans>CPU Usage</Trans>}
            icon={Cpu}
            content={`${health.cpu_usage.toFixed(1)}%`}
            description={<Trans>Total system load</Trans>}
            color="text-blue-500"
          />
          <MetricCard
            title={<Trans>Memory Usage</Trans>}
            icon={HardDrive}
            content={`${health.memory_usage.toFixed(1)}%`}
            description={<Trans>RAM utilization</Trans>}
            color="text-purple-500"
          />
        </motion.div>

        {/* Detailed Components Table */}
        <motion.div variants={item}>
          <Card className="overflow-hidden border-white/10 bg-background/30 backdrop-blur-xl shadow-2xl">
            <CardHeader>
              <CardTitle>
                <Trans>Component Status</Trans>
              </CardTitle>
              <CardDescription>
                <Trans>
                  Detailed health checks for internal services and resources.
                </Trans>
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>
                      <Trans>Component</Trans>
                    </TableHead>
                    <TableHead>
                      <Trans>Status</Trans>
                    </TableHead>
                    <TableHead>
                      <Trans>Message</Trans>
                    </TableHead>
                    <TableHead className="text-right">
                      <Trans>Latency</Trans>
                    </TableHead>
                    <TableHead className="text-right">
                      <Trans>Last Check</Trans>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {health.components
                    .sort((a, b) => a.name.localeCompare(b.name))
                    .map((component) => (
                      <TableRow
                        key={component.name}
                        className="hover:bg-white/5 transition-colors border-white/5"
                      >
                        <TableCell className="font-medium">
                          {formatComponentName(component.name, i18n)}
                          <div className="text-xs text-muted-foreground font-mono mt-0.5">
                            {component.name}
                          </div>
                        </TableCell>
                        <TableCell>
                          <HealthStatusBadge status={component.status} />
                        </TableCell>
                        <TableCell className="max-w-[400px]">
                          {component.message ? (
                            <span className="text-sm text-muted-foreground">
                              {component.message}
                            </span>
                          ) : (
                            <span className="text-xs text-muted-foreground/50 italic">
                              <Trans>No issues detected</Trans>
                            </span>
                          )}
                        </TableCell>
                        <TableCell className="text-right font-mono text-xs">
                          {component.check_duration_ms !== null &&
                          component.check_duration_ms !== undefined ? (
                            <span>{component.check_duration_ms}ms</span>
                          ) : (
                            <span className="text-muted-foreground/30">-</span>
                          )}
                        </TableCell>
                        <TableCell className="text-right text-xs text-muted-foreground">
                          {component.last_check && mounted
                            ? formatDistanceToNow(
                                new Date(component.last_check),
                                {
                                  addSuffix: true,
                                },
                              )
                            : component.last_check
                              ? format(new Date(component.last_check), 'PP p')
                              : '-'}
                        </TableCell>
                      </TableRow>
                    ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </motion.div>
      </motion.div>
    </div>
  );
}

function MetricCard({ title, icon: Icon, content, description, color }: any) {
  return (
    <Card className="border-white/10 bg-background/30 backdrop-blur-xl shadow-xl transition-all hover:bg-background/40 hover:scale-[1.02]">
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">
          {title}
        </CardTitle>
        <Icon className={cn('h-4 w-4 text-muted-foreground', color)} />
      </CardHeader>
      <CardContent>
        <div className="text-2xl font-bold">{content}</div>
        <p className="text-xs text-muted-foreground mt-1">{description}</p>
      </CardContent>
    </Card>
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

function formatComponentName(name: string, i18n: I18n): string {
  if (name.startsWith('disk:')) return i18n._(msg`Disk Space`);
  if (name === 'database') return i18n._(msg`Database`);
  if (name === 'download_manager') return i18n._(msg`Download Manager`);
  if (name === 'pipeline_manager') return i18n._(msg`Pipeline Manager`);
  if (name === 'danmu_service') return i18n._(msg`Danmu Service`);
  if (name === 'scheduler') return i18n._(msg`Scheduler`);
  if (name === 'notification_service') return i18n._(msg`Notification Service`);
  if (name === 'maintenance_scheduler') return i18n._(msg`Maintenance`);
  return name
    .split('_')
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}
