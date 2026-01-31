import { memo, useMemo } from 'react';
import { formatDistanceToNow } from 'date-fns';
import { cn } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';
import { Globe, Activity, Link2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { Button } from '@/components/ui/button';
import { EventIcon } from './event-icon';
import { PayloadPreview } from './payload-preview';

export const getPriorityStyles = (priority: string) => {
  switch (priority.toLowerCase()) {
    case 'critical':
      return {
        bg: 'bg-destructive/5 dark:bg-destructive/10',
        border: 'border-destructive/20 group-hover:border-destructive/40',
        text: 'text-destructive',
        badge: 'bg-destructive/10 text-destructive border-destructive/20',
        icon: 'bg-destructive/10 text-destructive',
        glow: 'group-hover:shadow-[0_0_20px_rgba(239,68,68,0.15)]',
        flare: 'from-destructive/20 to-transparent',
      };
    case 'high':
      return {
        bg: 'bg-orange-500/5 dark:bg-orange-500/10',
        border: 'border-orange-500/20 group-hover:border-orange-500/40',
        text: 'text-orange-600 dark:text-orange-400',
        badge:
          'bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20',
        icon: 'bg-orange-500/10 text-orange-600 dark:text-orange-400',
        glow: 'group-hover:shadow-[0_0_20px_rgba(249,115,22,0.15)]',
        flare: 'from-orange-500/20 to-transparent',
      };
    case 'normal':
      return {
        bg: 'bg-blue-500/5 dark:bg-blue-500/10',
        border: 'border-blue-500/20 group-hover:border-blue-500/40',
        text: 'text-blue-600 dark:text-blue-400',
        badge:
          'bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20',
        icon: 'bg-blue-500/10 text-blue-600 dark:text-blue-400',
        glow: 'group-hover:shadow-[0_0_20px_rgba(59,130,246,0.15)]',
        flare: 'from-blue-500/20 to-transparent',
      };
    case 'low':
      return {
        bg: 'bg-slate-500/5 dark:bg-slate-500/10',
        border: 'border-slate-500/20 group-hover:border-slate-500/40',
        text: 'text-slate-600 dark:text-slate-400',
        badge:
          'bg-slate-500/10 text-slate-600 dark:text-slate-400 border-slate-500/20',
        icon: 'bg-slate-500/10 text-slate-600 dark:text-slate-400',
        glow: 'group-hover:shadow-[0_0_20px_rgba(100,116,139,0.15)]',
        flare: 'from-slate-500/20 to-transparent',
      };
    default:
      return {
        bg: 'bg-slate-500/5 dark:bg-slate-500/10',
        border: 'border-slate-500/20 group-hover:border-slate-500/40',
        text: 'text-slate-600 dark:text-slate-400',
        badge:
          'bg-slate-500/10 text-slate-600 dark:text-slate-400 border-slate-500/20',
        icon: 'bg-slate-500/10 text-slate-600 dark:text-slate-400',
        glow: 'group-hover:shadow-[0_0_20px_rgba(100,116,139,0.15)]',
        flare: 'from-slate-500/20 to-transparent',
      };
  }
};

export interface NotificationEvent {
  id: string;
  event_type: string;
  priority: string;
  payload: string;
  created_at: number;
  streamer_id?: string | null;
  read?: boolean;
}

interface EventCardProps {
  event: NotificationEvent;
  onViewDetails: (event: NotificationEvent) => void;
}

export const EventCard = memo(({ event, onViewDetails }: EventCardProps) => {
  const styles = useMemo(
    () => getPriorityStyles(event.priority),
    [event.priority],
  );
  const displayTitle = event.event_type.replace(/_/g, ' ');

  return (
    <div
      className={cn(
        'group relative flex flex-col gap-4 p-5 rounded-[2rem] border transition-all duration-500 overflow-hidden',
        styles.bg,
        styles.border,
        styles.glow,
        event.read
          ? 'opacity-70 grayscale-[0.2]'
          : 'bg-card ring-1 ring-primary/5 shadow-sm',
      )}
    >
      {/* Background Flare */}
      <div
        className={cn(
          'absolute -top-12 -right-12 w-32 h-32 bg-gradient-to-br opacity-0 group-hover:opacity-100 transition-opacity duration-700 blur-2xl rounded-full pointer-events-none',
          styles.flare,
        )}
      />

      {!event.read && (
        <div
          className="absolute top-6 right-6 h-1.5 w-1.5 rounded-full bg-primary"
          style={{ filter: 'drop-shadow(0 0 4px hsl(var(--primary)))' }}
        />
      )}

      <div className="flex items-start justify-between">
        <div className="flex items-center gap-3">
          <div
            className={cn(
              'p-2.5 rounded-2xl ring-1 ring-inset ring-white/10 shadow-lg transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3',
              styles.icon,
            )}
          >
            <EventIcon eventType={event.event_type} className="h-5 w-5" />
          </div>
          <div>
            <div className="flex items-center gap-2 mb-0.5">
              <Badge
                variant="outline"
                className={cn(
                  'text-[9px] font-bold uppercase tracking-widest px-1.5 h-4 border-none shadow-none',
                  styles.badge,
                )}
              >
                {event.priority}
              </Badge>
            </div>
            <h4 className="text-[13px] font-bold leading-none tracking-tight text-foreground/90 group-hover:text-primary transition-colors">
              {displayTitle}
            </h4>
          </div>
        </div>

        <div className="flex flex-col items-end gap-1">
          <span className="text-[10px] font-bold text-muted-foreground/60 tabular-nums tracking-tighter uppercase">
            {formatDistanceToNow(new Date(event.created_at), {
              addSuffix: true,
            })}
          </span>
          {event.streamer_id ? (
            <div className="flex items-center gap-1 text-[9px] font-medium text-muted-foreground/40 bg-muted/20 px-1.5 py-0.5 rounded-full border border-border/10">
              <Globe className="h-2.5 w-2.5" />
              <span className="truncate max-w-[80px]">{event.streamer_id}</span>
            </div>
          ) : (
            <div className="flex items-center gap-1 text-[9px] font-medium text-muted-foreground/30 bg-muted/10 px-1.5 py-0.5 rounded-full">
              <Activity className="h-2.5 w-2.5" />
              <Trans>SYSTEM</Trans>
            </div>
          )}
        </div>
      </div>

      <div className="flex-1 space-y-4">
        <div className="relative">
          <PayloadPreview payload={event.payload} />
        </div>

        <div className="flex items-center justify-end pt-2 opacity-0 group-hover:opacity-100 translate-y-2 group-hover:translate-y-0 transition-all duration-300">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => onViewDetails(event)}
            className="h-8 gap-2 rounded-xl text-[10px] font-black uppercase tracking-widest hover:bg-primary/10 hover:text-primary"
          >
            <Trans>Interact</Trans>
            <Link2 className="h-3 w-3" />
          </Button>
        </div>
      </div>
    </div>
  );
});
EventCard.displayName = 'EventCard';
