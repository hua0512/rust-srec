import {
  startTransition,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react';
import { createLazyFileRoute, Link } from '@tanstack/react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  Bell,
  RefreshCw,
  Activity,
  X,
  CheckCheck,
  Filter,
  Flame,
  Zap,
  Circle,
} from 'lucide-react';

import { JsonViewer, prettyJson } from '@/components/shared/json-viewer';
import { EventCard } from '@/components/notifications/events/event-card';

import { listEvents, listEventTypes } from '@/server/functions/notifications';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
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
  onNotificationStateChanged,
  setLastSeenCriticalMs,
} from '@/lib/notification-state';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { SearchInput } from '@/components/sessions/search-input';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/notifications/events',
)({
  component: NotificationEventsPage,
});

// Redundant local components removed, now imported from components.

function NotificationEventsPage() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const [eventType, setEventType] = useState<string>('all');
  const [priority, setPriority] = useState<string>('all');
  const [streamerId, setStreamerId] = useState<string>('');
  const [page, setPage] = useState(1);
  const [selectedPayload, setSelectedPayload] = useState<{
    title: string;
    payload: string;
  } | null>(null);

  const limit = 50;

  const [lastSeenMs, setLastSeenMs] = useState(() => getLastSeenCriticalMs());

  useEffect(() => {
    return onNotificationStateChanged(() => {
      setLastSeenMs(getLastSeenCriticalMs());
    });
  }, []);

  const { data: eventTypes } = useQuery({
    queryKey: ['notification-event-types'],
    queryFn: () => listEventTypes(),
  });

  const query = useMemo(() => {
    return {
      limit,
      offset: (page - 1) * limit,
      event_type: eventType === 'all' ? undefined : eventType,
      priority: priority === 'all' ? undefined : priority,
      search:
        streamerId && streamerId !== 'all' && streamerId.trim()
          ? streamerId.trim()
          : undefined,
    };
  }, [eventType, priority, streamerId, page]);

  const {
    data: rawEvents,
    isLoading,
    error,
    refetch,
    isRefetching,
  } = useQuery({
    queryKey: ['notification-events', query],
    queryFn: () => listEvents({ data: query }),
    refetchInterval: 15000,
  });

  const events = useMemo(() => {
    if (!rawEvents) return [];
    return rawEvents.map((e) => ({
      ...e,
      read: e.created_at <= lastSeenMs,
    }));
  }, [rawEvents, lastSeenMs]);

  // Only auto-mark critical events as seen when they're actually visible
  useEffect(() => {
    if (!rawEvents || typeof window === 'undefined') return;

    // Don't auto-dismiss if filters would hide critical events
    const criticalVisible = priority === 'all' || priority === 'critical';
    if (!criticalVisible) return;

    const maxCritical = rawEvents.reduce((max, e) => {
      const p = (e.priority ?? '').toString().trim().toLowerCase();
      if (p !== 'critical') return max;
      return Math.max(max, e.created_at);
    }, 0);
    if (maxCritical <= 0) return;
    const prev = getLastSeenCriticalMs();
    if (maxCritical > prev) {
      setLastSeenCriticalMs(maxCritical);
      setLastSeenMs(maxCritical);
    }
  }, [rawEvents, priority]);

  const handleMarkAllRead = useCallback(() => {
    const now = Date.now();
    setLastSeenCriticalMs(now);
    setLastSeenMs(now);
    void queryClient.invalidateQueries({
      queryKey: ['notification-critical-dot'],
    });
    toast.success(i18n._(msg`Marked all as read`));
  }, [i18n, queryClient]);

  const priorityFilters = useMemo(
    () => [
      { value: 'all', label: i18n._(msg`All`), icon: Filter },
      {
        value: 'critical',
        label: i18n._(msg`Critical`),
        icon: Flame,
        color: 'text-red-500',
      },
      {
        value: 'high',
        label: i18n._(msg`High+`),
        icon: Zap,
        color: 'text-orange-500',
      },
      {
        value: 'normal',
        label: i18n._(msg`Normal+`),
        icon: Circle,
        color: 'text-blue-500',
      },
    ],
    [i18n],
  );

  const hasActiveFilters =
    eventType !== 'all' ||
    priority !== 'all' ||
    (streamerId !== '' && streamerId !== 'all');

  const clearFilters = useCallback(() => {
    startTransition(() => {
      setEventType('all');
      setPriority('all');
      setStreamerId('');
      setPage(1);
    });
  }, []);

  const handleSetPriority = useCallback((val: string) => {
    startTransition(() => {
      setPriority(val);
    });
  }, []);

  const handleSetEventType = useCallback((val: string) => {
    startTransition(() => {
      setEventType(val);
    });
  }, []);

  const handleSetStreamerId = useCallback((val: string) => {
    startTransition(() => {
      setStreamerId(val);
    });
  }, []);

  const handleSetPage = useCallback(
    (updater: number | ((p: number) => number)) => {
      setPage(updater);
    },
    [],
  );

  const handleViewDetails = useCallback((event: any) => {
    setSelectedPayload({
      title: `${event.event_type} (${event.priority})`,
      payload: event.payload,
    });
  }, []);

  useEffect(() => {
    setPage(1);
  }, [eventType, priority, streamerId]);

  const hasMore = (events?.length ?? 0) >= limit;
  const hasPrev = page > 1;

  if (error) {
    return (
      <div className="p-4 md:p-8 space-y-4">
        <div className="flex items-center gap-3">
          <div className="p-2.5 rounded-xl bg-linear-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
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
            {(error as any)?.message || i18n._(msg`Unknown error`)}
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
    <div className="min-h-screen space-y-6 bg-linear-to-br from-background via-background to-muted/20">
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
              onSearch={handleSetStreamerId}
              placeholder={i18n._(msg`Search streamer...`)}
              className="md:w-56 min-w-50"
            />

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            <div className="flex items-center bg-muted/30 p-1 rounded-full border border-border/50 shrink-0">
              {priorityFilters.map((filter) => {
                const Icon = filter.icon;
                const isActive = priority === filter.value;
                return (
                  <button
                    key={filter.value}
                    onClick={() => handleSetPriority(filter.value)}
                    className={cn(
                      'flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs font-medium transition-all duration-200',
                      isActive
                        ? 'bg-background text-foreground shadow-sm ring-1 ring-border/50'
                        : 'text-muted-foreground hover:text-foreground hover:bg-muted/50',
                    )}
                  >
                    <Icon
                      className={cn(
                        'h-3 w-3',
                        isActive
                          ? filter.color || 'text-primary'
                          : 'text-muted-foreground',
                      )}
                    />
                    <span>{filter.label}</span>
                  </button>
                );
              })}
            </div>

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            <div className="flex items-center bg-muted/30 p-1 rounded-full border border-border/50 shrink-0">
              <Select value={eventType} onValueChange={handleSetEventType}>
                <SelectTrigger className="h-7 border-none bg-transparent hover:bg-background/50 transition-colors rounded-full text-xs font-medium px-3 gap-1.5 focus:ring-0 shadow-none">
                  <Activity className="h-3.5 w-3.5 text-muted-foreground" />
                  <SelectValue placeholder={i18n._(msg`Event Type`)} />
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
              variant="ghost"
              size="sm"
              className="h-9 px-4 rounded-xl text-xs font-black uppercase tracking-widest bg-muted/30 text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-all duration-300 shrink-0"
              onClick={handleMarkAllRead}
            >
              <CheckCheck className="mr-2 h-3.5 w-3.5" />
              <Trans>Mark Read</Trans>
            </Button>

            <Button
              variant="ghost"
              size="sm"
              className="h-9 px-4 rounded-xl text-xs font-black uppercase tracking-widest bg-muted/30 text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-all duration-300 shrink-0"
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
              className="h-9 px-4 rounded-xl text-xs font-black uppercase tracking-widest bg-primary/10 text-primary border-primary/20 hover:bg-primary/20 transition-all duration-300 shrink-0"
            >
              <Link to="/notifications">
                <Trans>Channels</Trans>
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
            {events.map((e) => (
              <EventCard
                key={e.id}
                event={e}
                onViewDetails={handleViewDetails}
              />
            ))}
          </div>
        )}

        {(hasPrev || hasMore) && (
          <div className="flex items-center justify-center gap-2 pt-6">
            <Button
              variant="outline"
              size="sm"
              onClick={() => handleSetPage((p) => Math.max(1, p - 1))}
              disabled={!hasPrev}
              className="h-9 px-4 rounded-xl"
            >
              <Trans>Previous</Trans>
            </Button>
            <div className="flex items-center gap-1 px-4">
              <span className="text-sm text-muted-foreground">
                <Trans>Page</Trans>
              </span>
              <span className="text-sm font-medium">{page}</span>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => handleSetPage((p) => p + 1)}
              disabled={!hasMore}
              className="h-9 px-4 rounded-xl"
            >
              <Trans>Next</Trans>
            </Button>
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
            <DialogTitle className="flex items-center gap-2">
              <div className="p-1.5 rounded-lg bg-primary/10">
                <Activity className="h-4 w-4 text-primary" />
              </div>
              {selectedPayload?.title}
            </DialogTitle>
          </DialogHeader>
          <ScrollArea className="max-h-[70vh] rounded-xl bg-muted/50 p-4 border">
            {selectedPayload && <JsonViewer json={selectedPayload.payload} />}
          </ScrollArea>
          <div className="flex justify-end gap-2">
            <Button
              variant="outline"
              size="sm"
              className="rounded-xl"
              onClick={() => {
                if (!selectedPayload) return;
                try {
                  void navigator.clipboard.writeText(
                    prettyJson(selectedPayload.payload),
                  );
                  toast.success(i18n._(msg`Copied`));
                } catch {
                  toast.error(i18n._(msg`Failed to copy`));
                }
              }}
            >
              <Trans>Copy JSON</Trans>
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
