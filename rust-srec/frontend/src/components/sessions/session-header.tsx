import { Link } from '@tanstack/react-router';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';
import { ArrowLeft, Hash, Radio, CheckCircle2 } from 'lucide-react';

const STATUS_CONFIG: Record<
  string,
  {
    icon: any;
    color: string;
    badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline';
    animate?: boolean;
    gradient: string;
    borderColor: string;
    label: { id: string; message?: string };
  }
> = {
  LIVE: {
    icon: Radio,
    color: 'text-red-500',
    badgeVariant: 'default',
    animate: true,
    gradient: 'from-red-500/20 to-red-500/5',
    borderColor: 'border-red-500/20',
    label: msg`LIVE`,
  },
  OFFLINE: {
    icon: CheckCircle2,
    color: 'text-muted-foreground',
    badgeVariant: 'secondary',
    gradient: 'from-gray-500/20 to-gray-500/5',
    borderColor: 'border-gray-500/20',
    label: msg`OFFLINE`,
  },
};

interface SessionHeaderProps {
  session: {
    id: string;
    title?: string | null;
    end_time?: string | null;
  };
}
import { useLingui } from '@lingui/react';

export function SessionHeader({ session }: SessionHeaderProps) {
  const { i18n } = useLingui();
  const isLive = !session.end_time;
  const statusConfig = isLive ? STATUS_CONFIG.LIVE : STATUS_CONFIG.OFFLINE;
  const StatusIcon = statusConfig.icon;

  return (
    <div className="flex flex-col gap-6 md:gap-8 mb-8 md:mb-10">
      <motion.div
        initial={{ opacity: 0, x: -20 }}
        animate={{ opacity: 1, x: 0 }}
      >
        <Button
          variant="ghost"
          size="sm"
          asChild
          className="group text-muted-foreground hover:text-foreground hover:bg-transparent px-0"
        >
          <Link to="/sessions" className="flex items-center">
            <ArrowLeft className="mr-2 h-4 w-4 transition-transform group-hover:-translate-x-1" />
            <Trans>Back to Sessions</Trans>
          </Link>
        </Button>
      </motion.div>

      <div className="flex flex-col md:flex-row md:items-start justify-between gap-6">
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.1 }}
          className="flex items-start gap-5"
        >
          <div
            className={cn(
              'flex items-center justify-center w-14 h-14 md:w-16 md:h-16 rounded-xl md:rounded-2xl shadow-lg ring-1 ring-white/10 backdrop-blur-md bg-gradient-to-br shrink-0',
              statusConfig.gradient,
            )}
          >
            <StatusIcon
              className={cn(
                'h-8 w-8',
                statusConfig.color,
                statusConfig.animate && 'animate-pulse',
              )}
            />
          </div>
          <div>
            <div className="flex items-center gap-3 mb-1.5">
              <h1 className="text-2xl md:text-3xl font-bold tracking-tight text-foreground">
                <Trans>Session Details</Trans>
              </h1>
              <Badge
                variant="outline"
                className={cn(
                  'border bg-background/50 backdrop-blur font-mono text-[10px] md:text-xs uppercase tracking-wider h-5 md:h-6',
                  statusConfig.borderColor,
                  statusConfig.color,
                )}
              >
                {i18n._(statusConfig.label)}
              </Badge>
            </div>
            <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground font-medium">
              <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-muted/50 border border-border/50">
                <Hash className="h-3.5 w-3.5" />
                ID: {session.id}
              </span>
              {session.title && (
                <span className="text-xs opacity-75 truncate max-w-md">
                  {session.title}
                </span>
              )}
            </div>
          </div>
        </motion.div>
      </div>
    </div>
  );
}
