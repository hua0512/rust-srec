import { createFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  keepPreviousData,
  useQueryClient,
} from '@tanstack/react-query';
import { z } from 'zod';
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
  Square,
  Trash2,
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

const searchSchema = z.object({
  page: z.number().optional().catch(1),
  limit: z.number().optional().catch(50),
  streamer_id: z.string().optional(),
  search: z.string().optional(),
  status: z.enum(['all', 'active', 'completed']).optional().catch('all'),
  timeRange: z
    .enum(['all', 'today', 'yesterday', 'week', 'month', 'custom'])
    .optional()
    .catch('all'),
  from: z.string().optional(),
  to: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/sessions/')({
  component: SessionsPage,
  validateSearch: searchSchema,
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
  });

  const updateSearch = (newParams: Partial<typeof search>) => {
    navigate({
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

  // Animation variants matching dashboard
  const container = {
    hidden: { opacity: 0 },
    show: {
      opacity: 1,
      transition: {
        staggerChildren: 0.1,
      },
    },
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
      queryClient.invalidateQueries({ queryKey: ['sessions'] });
    } catch (error) {
      console.error('Failed to delete sessions:', error);
      toast.error(i18n._(msg`Failed to delete sessions`));
    } finally {
      setIsDeleting(false);
    }
  }, [selectedIds, i18n, queryClient]);

  return (
    <motion.div
      className="min-h-screen space-y-6 bg-gradient-to-br from-background via-background to-muted/20"
      variants={container}
      initial="hidden"
      animate="show"
    >
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

            <div className="h-6 w-px bg-border/50 mx-1 shrink-0" />

            {/* Selection Mode Toggle */}
            <Button
              variant="ghost"
              size="sm"
              onClick={toggleSelectionMode}
              className={cn(
                'h-9 px-4 rounded-xl text-xs font-black uppercase tracking-widest transition-all duration-500 shrink-0 border border-white/5',
                selectionMode
                  ? 'bg-primary text-primary-foreground border-primary/50 shadow-[0_0_20px_rgba(var(--primary),0.3)] hover:bg-primary/90'
                  : 'bg-muted/30 text-muted-foreground hover:text-foreground hover:bg-muted/50',
              )}
            >
              <motion.div
                animate={{ scale: selectionMode ? [1, 1.2, 1] : 1 }}
                transition={{ duration: 0.4 }}
                className="mr-2"
              >
                {selectionMode ? (
                  <CheckSquare className="h-4 w-4" />
                ) : (
                  <Square className="h-4 w-4 text-muted-foreground/50" />
                )}
              </motion.div>
              <Trans>Manage</Trans>
            </Button>
          </>
        }
      />

      {/* Batch Delete FAB (Bottom Floating Bar) */}
      <AnimatePresence>
        {selectionMode && (
          <motion.div
            initial={{ opacity: 0, y: 100, x: '-50%' }}
            animate={{ opacity: 1, y: 0, x: '-50%' }}
            exit={{ opacity: 0, y: 100, x: '-50%' }}
            className="fixed bottom-8 left-1/2 z-50 min-w-[320px] max-w-[90vw]"
          >
            <div className="relative overflow-hidden rounded-full border border-white/10 shadow-[0_25px_60px_rgba(0,0,0,0.6)] bg-background/40 backdrop-blur-3xl p-1.5 flex items-center gap-2">
              <div className="absolute inset-0 bg-gradient-to-br from-primary/20 via-transparent to-primary/10 opacity-30 pointer-events-none" />
              <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_100%,rgba(var(--primary),0.15),transparent_60%)] pointer-events-none" />

              {/* Selection Count Badge */}
              <div className="flex items-center gap-2.5 px-4 py-2 rounded-full bg-primary/20 border border-primary/30 shadow-[inset_0_1px_1px_rgba(255,255,255,0.1)] ml-1">
                <span className="text-sm font-black tabular-nums text-primary">
                  {selectedIds.size}
                </span>
                <span className="text-[10px] font-black uppercase tracking-widest text-primary/80 hidden sm:inline">
                  <Trans>Selected</Trans>
                </span>
              </div>
              {/* Bulk Action Buttons */}
              <div className="flex items-center gap-1.5 flex-1 justify-center px-1">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={handleSelectAll}
                  className="h-9 px-4 text-[10px] font-black uppercase tracking-wider hover:bg-white/10 rounded-full text-muted-foreground hover:text-foreground transition-all"
                >
                  <Trans>All</Trans>
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={handleDeselectAll}
                  disabled={selectedIds.size === 0}
                  className="h-9 px-4 text-[10px] font-black uppercase tracking-wider hover:bg-white/10 rounded-full text-muted-foreground hover:text-foreground transition-all"
                >
                  <Trans>None</Trans>
                </Button>
              </div>

              {/* Delete Button */}
              <Button
                variant="destructive"
                size="sm"
                onClick={handleBatchDelete}
                disabled={selectedIds.size === 0 || isDeleting}
                className={cn(
                  'h-10 px-6 rounded-full text-[10px] font-black uppercase tracking-[0.2em] transition-all duration-500 shadow-xl relative group/del overflow-hidden flex items-center justify-center shrink-0',
                  selectedIds.size > 0
                    ? 'bg-destructive shadow-destructive/20 hover:shadow-destructive/40 active:scale-95 translate-y-0 opacity-100'
                    : 'bg-muted text-muted-foreground/30 opacity-50 translate-y-1',
                )}
              >
                <div className="absolute inset-0 bg-gradient-to-r from-transparent via-white/10 to-transparent -translate-x-full group-hover/del:translate-x-full transition-transform duration-1000" />
                <Trash2 className="h-4 w-4 mr-2.5 group-hover/del:animate-bounce" />
                <span className="hidden sm:inline">
                  <Trans>Delete</Trans>
                </span>
              </Button>

              <div className="h-6 w-px bg-white/10 mx-1" />

              {/* Close/Exit Management Mode */}
              <Button
                variant="ghost"
                size="icon"
                onClick={toggleSelectionMode}
                className="h-10 w-10 rounded-full hover:bg-white/10 text-muted-foreground hover:text-foreground transition-all border border-white/5 shrink-0 mr-1"
              >
                <X className="h-5 w-5" />
              </Button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      <div className="p-4 md:px-8 space-y-6 pb-20">
        <SessionList
          sessions={query.data?.items || []}
          isLoading={query.isLoading}
          onRefresh={() => query.refetch()}
          token={user?.token?.access_token}
          selectionMode={selectionMode}
          selectedIds={selectedIds}
          onSelectionChange={handleSelectionChange}
        />

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
    </motion.div>
  );
}
