import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  keepPreviousData,
  useQueryClient,
} from '@tanstack/react-query';
import { listSessions, deleteSessions } from '@/server/functions/sessions';
import { SessionList } from '@/components/sessions/session-list';
import { startOfDay, subDays, format } from 'date-fns';
import { useMemo, useState, useCallback } from 'react';
import {
  Film,
  Filter,
  Activity,
  CheckCircle2,
  CalendarDays,
  X,
  CheckSquare,
  Trash2,
  LayoutGrid,
  RefreshCcw,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination';
import { cn } from '@/lib/utils';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Calendar } from '@/components/ui/calendar';
import { Button } from '@/components/ui/button';
import { motion, AnimatePresence } from 'motion/react';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { SearchInput } from '@/components/sessions/search-input';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';

export const Route = createLazyFileRoute('/_authed/_dashboard/sessions/')({
  component: SessionsPage,
});

function SessionsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { user } = Route.useRouteContext();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();
  const [isCalendarOpen, setIsCalendarOpen] = useState(false);

  // Selection mode state
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [isDeleting, setIsDeleting] = useState(false);

  // Calculate dates based on timeRange or use custom dates
  const { from_date, to_date } = useMemo(() => {
    if (search.timeRange === 'custom' && search.from) {
      return {
        from_date: search.from,
        to_date: search.to,
      };
    }

    const now = new Date();
    let start: Date | undefined;
    let end: Date | undefined;

    switch (search.timeRange) {
      case 'today':
        start = startOfDay(now);
        break;
      case 'yesterday':
        start = startOfDay(subDays(now, 1));
        end = startOfDay(now);
        break;
      case 'week':
        start = subDays(now, 7);
        break;
      case 'month':
        start = subDays(now, 30);
        break;
    }

    return {
      from_date: start?.toISOString(),
      to_date: end?.toISOString(),
    };
  }, [search.timeRange, search.from, search.to]);

  const activeOnly =
    search.status === 'active'
      ? true
      : search.status === 'completed'
        ? false
        : undefined;

  const query = useQuery({
    queryKey: [
      'sessions',
      search.page,
      search.limit,
      search.streamer_id,
      search.search,
      activeOnly,
      from_date,
      to_date,
    ],
    queryFn: () =>
      listSessions({
        data: {
          page: search.page,
          limit: search.limit,
          streamer_id: search.streamer_id,
          search: search.search,
          active_only: activeOnly,
          from_date,
          to_date,
        },
      }),
    placeholderData: keepPreviousData,
    staleTime: 30000,
  });

  const updateSearch = (newParams: Partial<typeof search>) => {
    void navigate({
      search: (prev) => ({ ...prev, ...newParams }),
      replace: true,
    });
  };

  const total = query.data?.total || 0;
  const limit = search.limit || 50;
  const totalPages = Math.ceil(total / limit) || 1;
  const currentPage = search.page || 1;

  // Generate page numbers for pagination
  const pageNumbers = useMemo(() => {
    const pages = [];
    if (totalPages <= 7) {
      for (let i = 1; i <= totalPages; i++) pages.push(i);
    } else {
      if (currentPage <= 4) {
        for (let i = 1; i <= 5; i++) pages.push(i);
        pages.push('...');
        pages.push(totalPages);
      } else if (currentPage >= totalPages - 3) {
        pages.push(1);
        pages.push('...');
        for (let i = totalPages - 4; i <= totalPages; i++) pages.push(i);
      } else {
        pages.push(1);
        pages.push('...');
        for (let i = currentPage - 1; i <= currentPage + 1; i++) pages.push(i);
        pages.push('...');
        pages.push(totalPages);
      }
    }
    return pages;
  }, [totalPages, currentPage]);

  const statusFilters = [
    { value: 'all', label: i18n._(msg`All`), icon: Filter },
    { value: 'active', label: i18n._(msg`Live`), icon: Activity },
    { value: 'completed', label: i18n._(msg`Done`), icon: CheckCircle2 },
  ];

  const timeFilters = [
    { value: 'all', label: i18n._(msg`All Time`) },
    { value: 'today', label: i18n._(msg`Today`) },
    { value: 'yesterday', label: i18n._(msg`Yesterday`) },
    { value: 'week', label: i18n._(msg`Week`) },
    { value: 'month', label: i18n._(msg`Month`) },
  ];

  const hasActiveFilters =
    search.streamer_id ||
    search.search ||
    (search.status && search.status !== 'all') ||
    (search.timeRange && search.timeRange !== 'all') ||
    search.from;

  const currentStatus = search.status || 'all';
  const currentTimeRange = search.timeRange || 'all';
  const dateRange = {
    from: search.from ? new Date(search.from) : undefined,
    to: search.to ? new Date(search.to) : undefined,
  };

  // Selection handlers
  const handleSelectionChange = useCallback((id: string, selected: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (selected) {
        next.add(id);
      } else {
        next.delete(id);
      }
      return next;
    });
  }, []);

  const handleSelectAll = useCallback(() => {
    const allIds = query.data?.items?.map((s) => s.id) || [];
    setSelectedIds(new Set(allIds));
  }, [query.data?.items]);

  const handleDeselectAll = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  const toggleSelectionMode = useCallback(() => {
    setSelectionMode((prev) => {
      if (prev) {
        setSelectedIds(new Set()); // Clear selection when exiting
      }
      return !prev;
    });
  }, []);

  const handleBatchDelete = useCallback(async () => {
    if (selectedIds.size === 0) return;

    if (
      !window.confirm(
        i18n._(
          msg`Are you sure you want to delete ${selectedIds.size} sessions? This action cannot be undone.`,
        ),
      )
    ) {
      return;
    }

    setIsDeleting(true);
    try {
      const result = await deleteSessions({ data: Array.from(selectedIds) });
      toast.success(
        i18n._(msg`Successfully deleted ${result.deleted} sessions`),
      );
      setSelectedIds(new Set());
      setSelectionMode(false);
      void queryClient.invalidateQueries({ queryKey: ['sessions'] });
    } catch (error) {
      console.error('Failed to delete sessions:', error);
      toast.error(i18n._(msg`Failed to delete sessions`));
    } finally {
      setIsDeleting(false);
    }
  }, [selectedIds, i18n, queryClient]);

  return (
    <div className="min-h-screen space-y-6 bg-gradient-to-br from-background via-background to-muted/20">
      {/* Header */}
      <DashboardHeader
        icon={Film}
        title={<Trans>Sessions</Trans>}
        subtitle={<Trans>Review recorded streams and manage archives</Trans>}
        actions={
          <>
            {/* Search */}
            <SearchInput
              defaultValue={search.search || ''}
              onSearch={(val) =>
                updateSearch({ search: val || undefined, page: 1 })
              }
              placeholder={i18n._(msg`Search sessions...`)}
              className="md:w-56 min-w-[200px]"
            />

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            {/* Status Pills */}
            <div className="flex items-center bg-muted/30 p-1 rounded-full border border-border/50 shrink-0">
              {statusFilters.map((filter) => {
                const Icon = filter.icon;
                const isActive = currentStatus === filter.value;
                return (
                  <button
                    key={filter.value}
                    onClick={() =>
                      updateSearch({ status: filter.value as any, page: 1 })
                    }
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
                        isActive ? 'text-primary' : 'text-muted-foreground',
                      )}
                    />
                    <span>{filter.label}</span>
                  </button>
                );
              })}
            </div>

            {/* Time Range */}
            <div className="flex items-center bg-muted/30 p-1 rounded-full border border-border/50 shrink-0">
              <Popover open={isCalendarOpen} onOpenChange={setIsCalendarOpen}>
                <PopoverTrigger asChild>
                  <button
                    className={cn(
                      'flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs font-medium transition-all duration-200',
                      currentTimeRange === 'custom' || dateRange?.from
                        ? 'bg-background text-foreground shadow-sm ring-1 ring-border/50'
                        : 'text-muted-foreground hover:text-foreground hover:bg-muted/50',
                    )}
                  >
                    <CalendarDays
                      className={cn(
                        'h-3 w-3',
                        currentTimeRange === 'custom' || dateRange?.from
                          ? 'text-primary'
                          : 'text-muted-foreground',
                      )}
                    />
                    <span>
                      {dateRange?.from
                        ? dateRange.to
                          ? `${format(dateRange.from, 'MMM dd')} - ${format(dateRange.to, 'MMM dd')} `
                          : format(dateRange.from, 'MMM dd')
                        : i18n._(msg`Custom`)}
                    </span>
                  </button>
                </PopoverTrigger>
                <PopoverContent className="w-auto p-0" align="end">
                  <Calendar
                    mode="range"
                    selected={dateRange}
                    onSelect={(range) => {
                      if (range?.from) {
                        updateSearch({
                          timeRange: 'custom',
                          from: range.from.toISOString(),
                          to: range.to?.toISOString(),
                          page: 1,
                        });
                      } else {
                        updateSearch({
                          timeRange: 'all',
                          from: undefined,
                          to: undefined,
                          page: 1,
                        });
                      }
                    }}
                    disabled={(date) =>
                      date > new Date() || date < new Date('1900-01-01')
                    }
                    numberOfMonths={2}
                    initialFocus
                    className="rounded-md border shadow-xs"
                  />
                </PopoverContent>
              </Popover>

              {timeFilters.map((filter) => {
                const isActive = currentTimeRange === filter.value;
                return (
                  <button
                    key={filter.value}
                    onClick={() => {
                      if (filter.value !== 'custom') {
                        updateSearch({
                          timeRange: filter.value as any,
                          from: undefined,
                          to: undefined,
                          page: 1,
                        });
                      } else {
                        updateSearch({
                          timeRange: filter.value as any,
                          page: 1,
                        });
                      }
                    }}
                    className={cn(
                      'flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs font-medium transition-all duration-200 whitespace-nowrap',
                      isActive
                        ? 'bg-background text-foreground shadow-sm ring-1 ring-border/50'
                        : 'text-muted-foreground hover:text-foreground hover:bg-muted/50',
                    )}
                  >
                    <span>{filter.label}</span>
                  </button>
                );
              })}
            </div>

            {hasActiveFilters && (
              <Button
                variant="ghost"
                size="icon"
                onClick={() => navigate({ search: {}, replace: true })}
                className="h-8 w-8 rounded-full hover:bg-destructive/10 hover:text-destructive shrink-0"
              >
                <X className="h-4 w-4" />
              </Button>
            )}
          </>
        }
      />

      {/* Selection FAB */}
      <div className="fixed bottom-6 right-6 z-50 flex flex-col items-end gap-3">
        {/* Expanded action bar — shown when selection mode is active */}
        <AnimatePresence>
          {selectionMode && (
            <motion.div
              initial={{ opacity: 0, scale: 0.8, y: 20 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.8, y: 20 }}
              transition={{ type: 'spring', stiffness: 400, damping: 25 }}
              className="flex items-center gap-1.5 rounded-full border border-border/50 bg-card/95 backdrop-blur-xl shadow-[0_8px_40px_rgba(0,0,0,0.2)] p-1.5"
            >
              {/* Count badge */}
              <motion.div
                key={selectedIds.size}
                initial={{ scale: 0.8 }}
                animate={{ scale: 1 }}
                className="flex items-center gap-2 px-3.5 py-1.5 rounded-full bg-primary/10 border border-primary/20"
              >
                <motion.span
                  key={selectedIds.size}
                  initial={{ y: -8, opacity: 0 }}
                  animate={{ y: 0, opacity: 1 }}
                  className="text-sm font-bold tabular-nums text-primary"
                >
                  {selectedIds.size}
                </motion.span>
                <span className="text-[10px] font-semibold uppercase tracking-wider text-primary/70 hidden sm:inline">
                  <Trans>Selected</Trans>
                </span>
              </motion.div>

              <Button
                variant="ghost"
                size="sm"
                onClick={handleSelectAll}
                className="h-8 px-3 text-xs font-medium rounded-full hover:bg-muted/80 text-muted-foreground"
              >
                <Trans>All</Trans>
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={handleDeselectAll}
                disabled={selectedIds.size === 0}
                className="h-8 px-3 text-xs font-medium rounded-full hover:bg-muted/80 text-muted-foreground"
              >
                <Trans>None</Trans>
              </Button>

              <div className="h-5 w-px bg-border/50" />

              <motion.div whileTap={{ scale: 0.92 }}>
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={handleBatchDelete}
                  disabled={selectedIds.size === 0 || isDeleting}
                  className="h-9 px-4 rounded-full text-xs font-semibold gap-1.5 disabled:opacity-40"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  <span className="hidden sm:inline">
                    <Trans>Delete</Trans>
                  </span>
                </Button>
              </motion.div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* FAB toggle button */}
        <motion.button
          onClick={toggleSelectionMode}
          whileHover={{ scale: 1.05 }}
          whileTap={{ scale: 0.9 }}
          className={cn(
            'h-14 w-14 rounded-full flex items-center justify-center shadow-lg transition-colors duration-300 border',
            selectionMode
              ? 'bg-primary text-primary-foreground border-primary/50 shadow-primary/25'
              : 'bg-card text-muted-foreground border-border/50 hover:text-foreground shadow-black/15',
          )}
        >
          <AnimatePresence mode="wait" initial={false}>
            {selectionMode ? (
              <motion.div
                key="close"
                initial={{ rotate: -90, opacity: 0 }}
                animate={{ rotate: 0, opacity: 1 }}
                exit={{ rotate: 90, opacity: 0 }}
                transition={{ duration: 0.2 }}
              >
                <X className="h-5 w-5" />
              </motion.div>
            ) : (
              <motion.div
                key="select"
                initial={{ rotate: 90, opacity: 0 }}
                animate={{ rotate: 0, opacity: 1 }}
                exit={{ rotate: -90, opacity: 0 }}
                transition={{ duration: 0.2 }}
              >
                <CheckSquare className="h-5 w-5" />
              </motion.div>
            )}
          </AnimatePresence>
        </motion.button>
      </div>

      <div className="p-4 md:px-8 space-y-6 pb-20">
        {query.isPending ? (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
            {[1, 2, 3, 4, 5, 6, 7, 8].map((i) => (
              <div
                key={i}
                className="flex flex-col h-full gap-6 py-6 bg-card border border-border/50 rounded-2xl overflow-hidden shadow-[0_2px_12px_rgba(0,0,0,0.08)] animate-pulse"
              >
                {/* Header — matches CardHeader p-3.5 pb-0 */}
                <div className="p-3.5 pb-0 flex flex-row gap-3 items-start">
                  <Skeleton className="h-10 w-10 rounded-full shrink-0" />
                  <div className="flex-1 min-w-0 pt-0.5 space-y-1.5">
                    <div className="flex items-center justify-between gap-2">
                      <Skeleton className="h-3 w-16" />
                      <Skeleton className="h-4 w-12 rounded-full" />
                    </div>
                    <Skeleton className="h-4 w-full" />
                    <Skeleton className="h-4 w-2/3" />
                  </div>
                </div>
                {/* Content — matches CardContent p-4 pt-1 pb-4 */}
                <div className="p-4 pt-3 pb-4 flex-1 space-y-2.5">
                  <div className="flex items-center gap-2">
                    <Skeleton className="h-3 w-3 rounded-sm" />
                    <Skeleton className="h-3 w-32" />
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="flex items-center gap-1.5">
                      <Skeleton className="h-3 w-3 rounded-sm" />
                      <Skeleton className="h-3 w-14" />
                    </div>
                    <div className="w-px h-2.5 bg-border/30" />
                    <div className="flex items-center gap-1.5">
                      <Skeleton className="h-3 w-3 rounded-sm" />
                      <Skeleton className="h-3 w-16" />
                    </div>
                  </div>
                </div>
                {/* Footer — matches CardFooter p-3 pt-0 */}
                <div className="p-3 pt-0 flex justify-between items-center">
                  <Skeleton className="h-9 w-28 rounded-xl" />
                  <Skeleton className="h-9 w-9 rounded-xl" />
                </div>
              </div>
            ))}
          </div>
        ) : (query.data?.items?.length ?? 0) > 0 ? (
          <SessionList
            sessions={query.data.items}
            token={user?.token?.access_token}
            selectionMode={selectionMode}
            selectedIds={selectedIds}
            onSelectionChange={handleSelectionChange}
          />
        ) : (
          <motion.div
            key="empty"
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            className="flex flex-col items-center justify-center py-32 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5 backdrop-blur-sm shadow-sm"
          >
            <div className="p-6 bg-primary/5 rounded-full ring-1 ring-primary/10">
              <LayoutGrid className="h-16 w-16 text-primary/60" />
            </div>
            <div className="space-y-2 max-w-md">
              <h3 className="font-semibold text-2xl tracking-tight">
                <Trans>No Archives Found</Trans>
              </h3>
              <p className="text-muted-foreground">
                <Trans>
                  Your digital library is currently empty. Start recording to
                  populate your sessions here.
                </Trans>
              </p>
            </div>
            <Button
              variant="outline"
              onClick={() => query.refetch()}
              className="mt-4"
            >
              <RefreshCcw className="mr-2 h-4 w-4" />
              <Trans>Refresh</Trans>
            </Button>
          </motion.div>
        )}

        {/* Pagination */}
        {totalPages > 1 && (
          <div className="mt-8">
            <Pagination>
              <PaginationContent>
                <PaginationItem>
                  <PaginationPrevious
                    onClick={() =>
                      currentPage > 1 && updateSearch({ page: currentPage - 1 })
                    }
                    className={
                      currentPage === 1
                        ? 'pointer-events-none opacity-50'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>

                {pageNumbers.map((page, idx) => (
                  <PaginationItem key={idx}>
                    {page === '...' ? (
                      <PaginationEllipsis />
                    ) : (
                      <PaginationLink
                        isActive={currentPage === page}
                        onClick={() => updateSearch({ page: page as number })}
                        className="cursor-pointer"
                      >
                        {page}
                      </PaginationLink>
                    )}
                  </PaginationItem>
                ))}

                <PaginationItem>
                  <PaginationNext
                    onClick={() =>
                      currentPage < totalPages &&
                      updateSearch({ page: currentPage + 1 })
                    }
                    className={
                      currentPage === totalPages
                        ? 'pointer-events-none opacity-50'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>
              </PaginationContent>
            </Pagination>
          </div>
        )}
      </div>
    </div>
  );
}
