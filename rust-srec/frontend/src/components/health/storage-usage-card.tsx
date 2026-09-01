import { useMemo } from 'react';
import { HardDrive } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { HealthStatusBadge } from '@/components/health/health-status-badge';
import { formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';
import type { ComponentHealth, DiskUsage } from '@/api/schemas/system';

/** One filesystem, plus every monitored path that lives on it. */
export interface DiskGroup {
  mountPoint: string;
  /** Monitored paths on this filesystem, sorted and deduplicated. */
  paths: string[];
  /** Worst status among the components sharing this filesystem. */
  status: string;
  usage: DiskUsage;
}

/** Higher wins when two components on one filesystem disagree. */
const STATUS_SEVERITY: Record<string, number> = {
  healthy: 0,
  unknown: 1,
  degraded: 2,
  unhealthy: 3,
};

function severity(status: string): number {
  return STATUS_SEVERITY[status.toLowerCase()] ?? 1;
}

/**
 * Collapse `disk:` components onto the filesystems they measure.
 *
 * The backend registers one probe per configured output root, so several
 * components can report the same filesystem with identical figures (the
 * recording folder and the database folder both under `/`, say). Grouping by
 * `mount_point` renders one bar per real disk, fullest first.
 */
export function groupDisksByMountPoint(
  components: ComponentHealth[],
): DiskGroup[] {
  const groups = new Map<string, DiskGroup>();

  for (const component of components) {
    const usage = component.disk;
    if (!usage) continue;

    const existing = groups.get(usage.mount_point);
    if (!existing) {
      groups.set(usage.mount_point, {
        mountPoint: usage.mount_point,
        paths: [usage.path],
        status: component.status,
        usage,
      });
      continue;
    }

    existing.paths.push(usage.path);
    if (severity(component.status) > severity(existing.status)) {
      existing.status = component.status;
      existing.usage = usage;
    }
  }

  for (const group of groups.values()) {
    group.paths = [...new Set(group.paths)].sort();
  }

  return [...groups.values()].sort(
    (a, b) => b.usage.used_percent - a.usage.used_percent,
  );
}

/**
 * Colour the bar from the component status rather than re-deriving the
 * warning and critical thresholds here — the backend owns those, and the bar
 * must never disagree with the badge next to it.
 */
function indicatorClass(status: string): string {
  switch (status.toLowerCase()) {
    case 'degraded':
      return 'bg-yellow-500';
    case 'unhealthy':
      return 'bg-red-500';
    default:
      return 'bg-green-500';
  }
}

export function StorageUsageCard({
  components,
}: {
  components: ComponentHealth[];
}) {
  const groups = useMemo(
    () => groupDisksByMountPoint(components),
    [components],
  );
  if (groups.length === 0) return null;

  return (
    <Card className="overflow-hidden border-white/10 bg-background/30 backdrop-blur-xl shadow-2xl">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <HardDrive className="h-4 w-4 text-muted-foreground" />
          <Trans>Storage</Trans>
        </CardTitle>
        <CardDescription>
          <Trans>Free space on the disks recordings are written to.</Trans>
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-6">
        {groups.map((group) => {
          const free = formatBytes(group.usage.available_bytes);
          const used = formatBytes(group.usage.used_bytes);
          const total = formatBytes(group.usage.total_bytes);
          const percent = group.usage.used_percent.toFixed(1);
          const paths = group.paths.join(', ');
          const isHealthy = group.status.toLowerCase() === 'healthy';

          return (
            <div key={group.mountPoint} className="space-y-2">
              <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                <span className="font-mono text-sm text-muted-foreground">
                  {group.mountPoint}
                </span>
                <div className="flex items-center gap-3">
                  {!isHealthy && <HealthStatusBadge status={group.status} />}
                  <span
                    className={cn(
                      'text-lg font-bold tabular-nums',
                      !isHealthy && 'text-yellow-600 dark:text-yellow-400',
                      group.status.toLowerCase() === 'unhealthy' &&
                        'text-red-600 dark:text-red-400',
                    )}
                  >
                    <Trans>{free} free</Trans>
                  </span>
                </div>
              </div>

              <Progress
                value={group.usage.used_percent}
                className="h-2"
                indicatorClassName={indicatorClass(group.status)}
              />

              <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 text-xs text-muted-foreground">
                <span className="tabular-nums">
                  <Trans>
                    {used} of {total} used ({percent}%)
                  </Trans>
                </span>
                {/* `min-w-0` lets the path list wrap instead of running past
                    the card edge; flex items refuse to shrink without it. */}
                {paths !== group.mountPoint && (
                  <span className="min-w-0 font-mono break-all sm:text-right">
                    {paths}
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </CardContent>
    </Card>
  );
}
