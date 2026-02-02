import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import {
  CheckCircle2,
  AlertTriangle,
  AlertCircle,
  HelpCircle,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';

export type HealthStatus = string;

interface HealthStatusBadgeProps {
  status: HealthStatus;
  className?: string;
}

export function HealthStatusBadge({
  status,
  className,
}: HealthStatusBadgeProps) {
  const normalizedStatus = status.toLowerCase();

  switch (normalizedStatus) {
    case 'healthy':
      return (
        <Badge
          variant="outline"
          className={cn(
            'bg-green-500/10 text-green-600 dark:text-green-400 border-green-500/20 gap-1.5',
            className,
          )}
        >
          <CheckCircle2 className="h-3.5 w-3.5" />
          <Trans>Healthy</Trans>
        </Badge>
      );
    case 'degraded':
      return (
        <Badge
          variant="outline"
          className={cn(
            'bg-yellow-500/10 text-yellow-600 dark:text-yellow-400 border-yellow-500/20 gap-1.5',
            className,
          )}
        >
          <AlertTriangle className="h-3.5 w-3.5" />
          <Trans>Degraded</Trans>
        </Badge>
      );
    case 'unhealthy':
      return (
        <Badge
          variant="outline"
          className={cn(
            'bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20 gap-1.5',
            className,
          )}
        >
          <AlertCircle className="h-3.5 w-3.5" />
          <Trans>Unhealthy</Trans>
        </Badge>
      );
    default:
      return (
        <Badge
          variant="outline"
          className={cn('bg-muted text-muted-foreground gap-1.5', className)}
        >
          <HelpCircle className="h-3.5 w-3.5" />
          <span className="capitalize">{status}</span>
        </Badge>
      );
  }
}
