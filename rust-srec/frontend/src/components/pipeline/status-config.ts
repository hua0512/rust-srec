import {
  Clock,
  RefreshCw,
  CheckCircle2,
  XCircle,
  AlertCircle,
} from 'lucide-react';
import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { DagStatus, DagStepStatus } from '@/api/schemas';

export interface StatusConfigItem {
  icon: React.ElementType;
  textColor: string;
  bgColor: string;
  badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline';
  animate?: boolean;
  gradient: string;
  borderColor: string;
  surfaceBg: string;
  glow: string;
}

export const STATUS_CONFIG: Record<
  DagStatus | DagStepStatus,
  StatusConfigItem
> = {
  PENDING: {
    icon: Clock,
    textColor: 'text-muted-foreground',
    bgColor: 'bg-muted text-muted-foreground',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
    borderColor: 'border-gray-500/20',
    surfaceBg: 'bg-muted/10',
    glow: 'shadow-transparent',
  },
  PROCESSING: {
    icon: RefreshCw,
    textColor: 'text-blue-500',
    bgColor: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
    badgeVariant: 'default',
    animate: true,
    gradient: 'from-blue-500/20 to-blue-500/5',
    borderColor: 'border-blue-500/20',
    surfaceBg: 'bg-blue-500/10',
    glow: 'shadow-blue-500/20',
  },
  COMPLETED: {
    icon: CheckCircle2,
    textColor: 'text-emerald-500',
    bgColor: 'bg-green-500/10 text-green-500 border-green-500/20',
    badgeVariant: 'secondary',
    gradient: 'from-emerald-500/20 to-emerald-500/5',
    borderColor: 'border-emerald-500/20',
    surfaceBg: 'bg-emerald-500/10',
    glow: 'shadow-emerald-500/10',
  },
  FAILED: {
    icon: XCircle,
    textColor: 'text-red-500',
    bgColor: 'bg-red-500/10 text-red-500 border-red-500/20',
    badgeVariant: 'destructive',
    gradient: 'from-red-500/20 to-red-500/5',
    borderColor: 'border-red-500/20',
    surfaceBg: 'bg-red-500/10',
    glow: 'shadow-red-500/20',
  },
  CANCELLED: {
    icon: AlertCircle,
    textColor: 'text-gray-500',
    bgColor: 'bg-gray-500/10 text-gray-500 border-gray-500/20',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
    borderColor: 'border-gray-500/20',
    surfaceBg: 'bg-gray-500/10',
    glow: 'shadow-transparent',
  },
  BLOCKED: {
    icon: Clock,
    textColor: 'text-muted-foreground/40',
    bgColor: 'bg-muted/5 text-muted-foreground/40',
    badgeVariant: 'outline',
    gradient: 'from-gray-500/10 to-gray-500/5',
    borderColor: 'border-white/5',
    surfaceBg: 'bg-muted/5',
    glow: 'shadow-transparent',
  },
};

export function getStatusConfig(status: string): StatusConfigItem {
  return (
    STATUS_CONFIG[status as keyof typeof STATUS_CONFIG] ?? STATUS_CONFIG.PENDING
  );
}

export function getStatusLabel(i18n: I18n, status: string): string {
  switch (status) {
    case 'BLOCKED':
      return i18n._(msg`Blocked`);
    case 'PENDING':
      return i18n._(msg`Pending`);
    case 'PROCESSING':
      return i18n._(msg`Processing`);
    case 'COMPLETED':
      return i18n._(msg`Completed`);
    case 'FAILED':
      return i18n._(msg`Failed`);
    case 'CANCELLED':
      return i18n._(msg`Cancelled`);
    default:
      return status;
  }
}
