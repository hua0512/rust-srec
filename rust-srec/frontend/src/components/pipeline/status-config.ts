import {
  Clock,
  RefreshCw,
  CheckCircle2,
  XCircle,
  AlertCircle,
} from 'lucide-react';
import type { DagStatus, DagStepStatus } from '@/api/schemas';

export interface StatusConfigItem {
  icon: React.ElementType;
  /** Text color class (e.g., 'text-blue-500') */
  textColor: string;
  /** Background color class for cards (e.g., 'bg-blue-500/10 text-blue-500 border-blue-500/20') */
  bgColor: string;
  badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline';
  animate?: boolean;
  /** Gradient class for backgrounds (e.g., 'from-blue-500/20 to-blue-500/5') */
  gradient: string;
}

export const STATUS_CONFIG: Record<
  DagStatus | DagStepStatus | 'MIXED',
  StatusConfigItem
> = {
  PENDING: {
    icon: Clock,
    textColor: 'text-muted-foreground',
    bgColor: 'bg-muted text-muted-foreground',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
  },
  PROCESSING: {
    icon: RefreshCw,
    textColor: 'text-blue-500',
    bgColor: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
    badgeVariant: 'default',
    animate: true,
    gradient: 'from-blue-500/20 to-blue-500/5',
  },
  COMPLETED: {
    icon: CheckCircle2,
    textColor: 'text-emerald-500',
    bgColor: 'bg-green-500/10 text-green-500 border-green-500/20',
    badgeVariant: 'secondary',
    gradient: 'from-emerald-500/20 to-emerald-500/5',
  },
  FAILED: {
    icon: XCircle,
    textColor: 'text-red-500',
    bgColor: 'bg-red-500/10 text-red-500 border-red-500/20',
    badgeVariant: 'destructive',
    gradient: 'from-red-500/20 to-red-500/5',
  },
  INTERRUPTED: {
    icon: AlertCircle,
    textColor: 'text-orange-500',
    bgColor: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
    badgeVariant: 'secondary',
    gradient: 'from-orange-500/20 to-orange-500/5',
  },
  CANCELLED: {
    icon: AlertCircle,
    textColor: 'text-gray-500',
    bgColor: 'bg-gray-500/10 text-gray-500 border-gray-500/20',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
  },
  BLOCKED: {
    icon: Clock,
    textColor: 'text-muted-foreground/40',
    bgColor: 'bg-muted/5 text-muted-foreground/40',
    badgeVariant: 'outline',
    gradient: 'from-gray-500/10 to-gray-500/5',
  },
  MIXED: {
    icon: AlertCircle,
    textColor: 'text-orange-500',
    bgColor: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
    badgeVariant: 'secondary',
    gradient: 'from-orange-500/20 to-orange-500/5',
  },
};

/** Get status config with fallback to PENDING */
export function getStatusConfig(status: string): StatusConfigItem {
  return (
    STATUS_CONFIG[status as keyof typeof STATUS_CONFIG] ?? STATUS_CONFIG.PENDING
  );
}
