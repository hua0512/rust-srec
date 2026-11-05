import { msg } from '@lingui/core/macro';
import type { MessageDescriptor } from '@lingui/core';
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  HelpCircle,
  type LucideIcon,
} from 'lucide-react';

/**
 * How to present one `HealthStatus` value from `rust-srec/src/metrics/health.rs`.
 * `badge` styles an outline badge; `dot` fills a status dot or a progress bar.
 */
interface HealthStatusMeta {
  label: MessageDescriptor;
  icon: LucideIcon;
  badge: string;
  dot: string;
}

const UNKNOWN: HealthStatusMeta = {
  label: msg`Unknown`,
  icon: HelpCircle,
  badge: 'bg-muted text-muted-foreground',
  dot: 'bg-muted-foreground',
};

const HEALTH_STATUSES: Record<string, HealthStatusMeta> = {
  healthy: {
    label: msg`Healthy`,
    icon: CheckCircle2,
    badge:
      'bg-green-500/10 text-green-600 dark:text-green-400 border-green-500/20',
    dot: 'bg-green-500',
  },
  degraded: {
    label: msg`Degraded`,
    icon: AlertTriangle,
    badge:
      'bg-yellow-500/10 text-yellow-600 dark:text-yellow-400 border-yellow-500/20',
    dot: 'bg-yellow-500',
  },
  unhealthy: {
    label: msg`Unhealthy`,
    icon: AlertCircle,
    badge: 'bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20',
    dot: 'bg-red-500',
  },
  unknown: UNKNOWN,
};

/**
 * Case-insensitive. A status the table does not list presents as unknown, so
 * no view reports it as healthy or failing.
 */
export function getHealthStatusMeta(status: string): HealthStatusMeta {
  return HEALTH_STATUSES[status.toLowerCase()] ?? UNKNOWN;
}
