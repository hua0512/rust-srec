import { useLingui } from '@lingui/react';
import { Badge } from '@/components/ui/badge';
import { getHealthStatusMeta } from '@/lib/health-status';
import { cn } from '@/lib/utils';

interface HealthStatusBadgeProps {
  status: string;
  className?: string;
}

export function HealthStatusBadge({
  status,
  className,
}: HealthStatusBadgeProps) {
  const { i18n } = useLingui();
  const meta = getHealthStatusMeta(status);
  const Icon = meta.icon;

  return (
    <Badge variant="outline" className={cn(meta.badge, 'gap-1.5', className)}>
      <Icon className="h-3.5 w-3.5" />
      {i18n._(meta.label)}
    </Badge>
  );
}
