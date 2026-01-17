import { useEffect, useMemo, useState } from 'react';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { formatDistanceToNow, parseISO } from 'date-fns';
import {
  Bell,
  RefreshCw,
  Globe,
  Download,
  AlertCircle,
  AlertTriangle,
  Info,
  Settings,
  ShieldCheck,
  Webhook,
  Mail,
  MessageSquare,
  Activity,
  History,
  Timer,
  Clock,
  Layers,
  Link2,
  X,
  ListOrdered,
} from 'lucide-react';

import { listEvents, listEventTypes } from '@/server/functions/notifications';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { ScrollArea } from '@/components/ui/scroll-area';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import {
  getLastSeenCriticalMs,
  setLastSeenCriticalMs,
} from '@/lib/notification-state';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { SearchInput } from '@/components/sessions/search-input';

export const Route = createFileRoute(
  '/_authed/_dashboard/notifications/events',
)({
  component: NotificationEventsPage,
});

function EventIcon({
  eventType,
  className,
}: {
  eventType: string;
  className?: string;
}) {
  const type = eventType.toLowerCase();
  if (type.includes('download')) return <Download className={className} />;
  if (type.includes('error') || type.includes('fail'))
    return <AlertCircle className={className} />;
  if (type.includes('warning')) return <AlertTriangle className={className} />;
  if (type.includes('config') || type.includes('settings'))
    return <Settings className={className} />;
  if (type.includes('auth')) return <ShieldCheck className={className} />;
  if (type.includes('webhook')) return <Webhook className={className} />;
  if (type.includes('email')) return <Mail className={className} />;
  if (type.includes('danmu') || type.includes('chat'))
    return <MessageSquare className={className} />;
  if (type.includes('pipeline')) return <Layers className={className} />;
  if (type.includes('engine')) return <Activity className={className} />;
  if (type.includes('retention')) return <History className={className} />;
  if (type.includes('delay') || type.includes('timer'))
    return <Timer className={className} />;
  if (type.includes('recording') || type.includes('session'))
    return <Clock className={className} />;

  return <Info className={className} />;
}

function getPriorityStyles(priority: string) {
  const p = priority.toLowerCase();
  switch (p) {
    case 'critical':
      return {
        card: 'border-l-red-500 bg-red-500/5 hover:bg-red-500/10',
        glow: 'shadow-[0_0_15px_rgba(239,68,68,0.1)]',
        icon: 'bg-red-500/20 text-red-500',
        badge: 'bg-red-500/10 text-red-600 border-red-500/20',
      };
    case 'high':
      return {
        card: 'border-l-orange-500 bg-orange-500/5 hover:bg-orange-500/10',
        glow: 'shadow-[0_0_15px_rgba(249,115,22,0.1)]',
        icon: 'bg-orange-500/20 text-orange-500',
        badge: 'bg-orange-500/10 text-orange-600 border-orange-500/20',
      };
    case 'normal':
      return {
        card: 'border-l-blue-500 bg-blue-500/5 hover:bg-blue-500/10',
        glow: 'shadow-[0_0_15px_rgba(59,130,246,0.1)]',
        icon: 'bg-blue-500/20 text-blue-500',
        badge: 'bg-blue-500/10 text-blue-600 border-blue-500/20',
      };
    default:
      return {
        card: 'border-l-slate-500 bg-slate-500/5 hover:bg-slate-500/10',
        glow: 'shadow-[0_0_15px_rgba(100,116,139,0.1)]',
        icon: 'bg-slate-500/20 text-slate-500',
        badge: 'bg-slate-500/10 text-slate-600 border-slate-500/20',
      };
  }
}

function NotificationEventsPage() {
  const [eventType, setEventType] = useState<string>('all');
  const [streamerId, setStreamerId] = useState<string>('');
  const [limit, setLimit] = useState<string>('200');
  const [selectedPayload, setSelectedPayload] = useState<{
    title: string;
    payload: string;
  } | null>(null);

  const { data: eventTypes } = useQuery({
    queryKey: ['notification-event-types'],
    queryFn: () => listEventTypes(),
  });

  const query = useMemo(() => {
    const n = Number(limit);
    return {
      limit: Number.isFinite(n) ? n : 200,
      event_type: eventType === 'all' ? undefined : eventType,
      search:
        streamerId && streamerId !== 'all' && streamerId.trim()
          ? streamerId.trim()
          : undefined,
    };
  }, [eventType, streamerId, limit]);

  const {
    data: events,
    isLoading,
    error,
    refetch,
    isRefetching,
  } = useQuery({
    queryKey: ['notification-events', query],
    queryFn: () => listEvents({ data: query }),
    refetchInterval: 15000,
  });

  useEffect(() => {
    if (!events || typeof window === 'undefined') return;
    const maxCritical = events.reduce((max, e) => {
      const p = (e.priority ?? '').toString().trim().toLowerCase();
      if (p !== 'critical') return max;
      const ts = Date.parse(e.created_at);
      return Number.isFinite(ts) ? Math.max(max, ts) : max;
    }, 0);
    if (maxCritical <= 0) return;
    const prev = getLastSeenCriticalMs();
    if (maxCritical > prev) setLastSeenCriticalMs(maxCritical);
  }, [events]);

  const hasActiveFilters =
    eventType !== 'all' ||
    (streamerId !== '' && streamerId !== 'all') ||
    limit !== '200';

  const clearFilters = () => {
    setEventType('all');
    setStreamerId('');
    setLimit('200');
  };

  if (error) {
    return (
      <div className="p-4 md:p-8 space-y-4">
        <div className="flex items-center gap-3">
          <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
            <Bell className="h-5 w-5 text-primary" />
          </div>
          <div>
            <h1 className="text-xl font-semibold tracking-tight">
              <Trans>Notification Events</Trans>
            </h1>
            <p className="text-sm text-muted-foreground">
              <Trans>Recent notification events persisted by the backend</Trans>
            </p>
          </div>
        </div>

        <div className="rounded-xl border p-4 text-sm">
          <div className="font-medium">
            <Trans>Failed to load events</Trans>
          </div>
          <div className="text-muted-foreground">
            {(error as any)?.message || t`Unknown error`}
          </div>
          <div className="mt-3">
            <Button variant="outline" onClick={() => refetch()}>
              <RefreshCw className="mr-2 h-4 w-4" />
              <Trans>Try Again</Trans>
            </Button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen space-y-6 bg-gradient-to-br from-background via-background to-muted/20">
      <DashboardHeader
        icon={Bell}
        title={<Trans>Notification Events</Trans>}
        subtitle={
          <Trans>Recent notification events persisted by the backend</Trans>
        }
        actions={
          <>
            <SearchInput
              defaultValue={streamerId}
              onSearch={setStreamerId}
              placeholder={t`Filter streamer...`}
              className="md:w-56 min-w-[200px]"
            />

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            <div className="flex items-center bg-muted/30 p-1 rounded-full border border-border/50 shrink-0">
              <Select value={eventType} onValueChange={setEventType}>
                <SelectTrigger className="h-7 border-none bg-transparent hover:bg-background/50 transition-colors rounded-full text-xs font-medium px-3 gap-1.5 focus:ring-0 shadow-none">
                  <Activity className="h-3.5 w-3.5 text-muted-foreground" />
                  <SelectValue placeholder={t`Event Type`} />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    <Trans>All Types</Trans>
                  </SelectItem>
                  {(eventTypes ?? []).map((et) => (
                    <SelectItem key={et.event_type} value={et.event_type}>
                      {et.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            <div className="flex items-center bg-muted/30 p-1 rounded-full border border-border/50 shrink-0">
              <Select value={limit} onValueChange={setLimit}>
                <SelectTrigger className="h-7 border-none bg-transparent hover:bg-background/50 transition-colors rounded-full text-xs font-medium px-3 gap-1.5 focus:ring-0 shadow-none">
                  <ListOrdered className="h-3 w-3 text-muted-foreground" />
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {['50', '100', '200', '500', '1000'].map((v) => (
                    <SelectItem key={v} value={v}>
                      {v} items
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {hasActiveFilters && (
              <Button
                variant="ghost"
                size="icon"
                onClick={clearFilters}
                className="h-8 w-8 rounded-full hover:bg-destructive/10 hover:text-destructive shrink-0"
              >
                <X className="h-4 w-4" />
              </Button>
            )}

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            <Button
              variant="outline"
              size="sm"
              className="h-9 px-4 rounded-xl text-xs font-black uppercase tracking-widest bg-muted/30 text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-all duration-300"
              onClick={() => refetch()}
              disabled={isRefetching}
            >
              <RefreshCw
                className={cn(
                  'mr-2 h-3.5 w-3.5',
                  isRefetching && 'animate-spin',
                )}
              />
              <Trans>Refresh</Trans>
            </Button>

            <Button
              variant="outline"
              size="sm"
              asChild
              className="h-9 px-4 rounded-xl text-xs font-black uppercase tracking-widest bg-primary/10 text-primary border-primary/20 hover:bg-primary/20 transition-all duration-300"
            >
              <Link to="/notifications">
                <Trans>Notification Channels</Trans>
              </Link>
            </Button>
          </>
        }
      />

      <div className="p-4 md:px-8 pb-20 space-y-4">
        {isLoading ? (
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
            {[1, 2, 3, 4, 5, 6].map((i) => (
              <Skeleton key={i} className="h-32 w-full rounded-2xl" />
            ))}
          </div>
        ) : (events ?? []).length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20 text-center space-y-4 border-2 border-dashed rounded-3xl bg-muted/5">
            <div className="p-4 bg-muted/20 rounded-full">
              <Bell className="h-10 w-10 text-muted-foreground/40" />
            </div>
            <div className="space-y-1">
              <h3 className="font-medium text-lg">
                <Trans>No events found</Trans>
              </h3>
              <p className="text-sm text-muted-foreground">
                <Trans>Try adjusting your filters or limit</Trans>
              </p>
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
            {(events ?? []).map((e) => {
              const when = (() => {
                try {
                  return formatDistanceToNow(parseISO(e.created_at), {
                    addSuffix: true,
                  });
                } catch {
                  return e.created_at;
                }
              })();

              const styles = getPriorityStyles(e.priority);
              const isCrit = e.priority.toLowerCase() === 'critical';

              return (
                <div
                  key={e.id}
                  className={cn(
                    'group relative flex flex-col p-4 rounded-2xl border border-border/40 border-l-4 transition-all duration-300 hover:shadow-xl hover:scale-[1.02] backdrop-blur-sm',
                    styles.card,
                    styles.glow,
                  )}
                >
                  <div className="flex items-start justify-between mb-3">
                    <div className="flex items-center gap-3">
                      <div
                        className={cn(
                          'p-2 rounded-xl transition-colors duration-300 group-hover:scale-110',
                          styles.icon,
                        )}
                      >
                        <EventIcon
                          eventType={e.event_type}
                          className="h-4 w-4"
                        />
                      </div>
                      <div>
                        <div className="text-xs font-mono font-medium opacity-70 group-hover:opacity-100 transition-opacity">
                          {e.event_type}
                        </div>
                        <div className="text-[10px] text-muted-foreground flex items-center gap-1.5 mt-0.5">
                          <Activity className="h-2.5 w-2.5 opacity-50" />
                          {when}
                        </div>
                      </div>
                    </div>
                    <Badge
                      variant="outline"
                      className={cn(
                        'text-[9px] px-1.5 py-0 h-4 border-none shadow-none uppercase font-bold tracking-wider',
                        styles.badge,
                      )}
                    >
                      {e.priority}
                    </Badge>
                  </div>

                  <div className="flex-1">
                    {e.streamer_id && (
                      <div className="flex items-center gap-1.5 mb-2 px-2 py-1 rounded-lg bg-background/40 border border-border/20 w-fit">
                        <Globe className="h-3 w-3 text-primary/60" />
                        <span className="text-[10px] font-mono text-muted-foreground truncate max-w-[150px]">
                          {e.streamer_id}
                        </span>
                      </div>
                    )}
                    <p className="text-xs text-muted-foreground line-clamp-2 leading-relaxed">
                      {formatSummary(e.payload, e.event_type)}
                    </p>
                  </div>

                  <div className="mt-4 pt-3 border-t border-border/20 flex justify-between items-center opacity-0 group-hover:opacity-100 transition-opacity translate-y-2 group-hover:translate-y-0 duration-300">
                    <div className="flex items-center gap-2">
                      <div className="h-1.5 w-1.5 rounded-full bg-primary/40 animate-pulse" />
                      <span className="text-[9px] text-muted-foreground font-medium uppercase tracking-tighter">
                        <Trans>Details Ready</Trans>
                      </span>
                    </div>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-7 text-[10px] font-bold gap-1.5 hover:bg-primary/10 hover:text-primary transition-colors"
                      onClick={() => {
                        setSelectedPayload({
                          title: `${e.event_type} (${e.priority})`,
                          payload: e.payload,
                        });
                      }}
                    >
                      <Trans>View Details</Trans>
                      <Link2 className="h-3 w-3" />
                    </Button>
                  </div>

                  {/* Aesthetic Corner Flare */}
                  <div
                    className={cn(
                      'absolute top-0 right-0 w-16 h-16 bg-gradient-to-br from-white/10 to-transparent opacity-0 group-hover:opacity-100 transition-opacity rounded-tr-2xl pointer-events-none',
                      isCrit && 'from-red-500/20',
                    )}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>

      <Dialog
        open={!!selectedPayload}
        onOpenChange={(open) => {
          if (!open) setSelectedPayload(null);
        }}
      >
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>{selectedPayload?.title}</DialogTitle>
          </DialogHeader>
          <ScrollArea className="max-h-[70vh] rounded-lg bg-muted/30 p-3 border">
            <pre className="whitespace-pre-wrap text-xs font-mono">
              {prettyJson(selectedPayload?.payload)}
            </pre>
          </ScrollArea>
          <div className="flex justify-end gap-2">
            <Button
              variant="outline"
              onClick={() => {
                try {
                  navigator.clipboard.writeText(
                    prettyJson(selectedPayload?.payload),
                  );
                  toast.success(t`Copied`);
                } catch {
                  toast.error(t`Failed to copy`);
                }
              }}
            >
              <Trans>Copy</Trans>
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function formatSummary(payload: string, eventType: string): string {
  try {
    const parsed = JSON.parse(payload);
    const variant = Object.keys(parsed)[0];
    const inner = variant ? (parsed as any)[variant] : null;

    if (!inner) return payload.slice(0, 100);

    const streamer = inner.streamer_name || inner.streamer || inner.streamer_id;
    const error =
      inner.error_message || inner.error || inner.reason || inner.message;

    if (eventType.toLowerCase().includes('recording')) {
      if (variant === 'Started')
        return t`Recording started for ${streamer || 'unknown streamer'}`;
      if (variant === 'Finished')
        return t`Recording finished for ${streamer || 'unknown streamer'}`;
      if (variant === 'Failed')
        return t`Recording failed: ${error || 'Unknown error'}`;
    }

    if (eventType.toLowerCase().includes('download')) {
      if (variant === 'Started')
        return t`Download started: ${inner.title || streamer || 'unknown'}`;
      if (variant === 'Finished')
        return t`Download completed: ${inner.path || inner.title || 'unknown'}`;
      if (variant === 'Failed')
        return t`Download failed: ${error || 'Unknown error'}`;
    }

    if (error) return error;

    return (
      inner.message ||
      inner.reason ||
      (typeof inner === 'string' ? inner : JSON.stringify(inner).slice(0, 100))
    );
  } catch {
    return payload.slice(0, 100);
  }
}

function prettyJson(payload?: string) {
  if (!payload) return '';
  try {
    const parsed = JSON.parse(payload);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return payload;
  }
}
