import { createFileRoute } from '@tanstack/react-router';
import { useQuery, keepPreviousData } from '@tanstack/react-query';
import { z } from 'zod';
import { listSessions } from '@/server/functions/sessions';
import { SessionList } from '@/components/sessions/SessionList';
import { startOfDay, subDays, format } from 'date-fns';
import { useMemo, useState } from 'react';
import {
  Film,
  Search,
  Filter,
  Activity,
  CheckCircle2,
  CalendarDays,
  X,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Calendar } from '@/components/ui/calendar';
import { Button } from '@/components/ui/button';
import { motion } from 'motion/react';

const searchSchema = z.object({
  page: z.number().optional().catch(1),
  limit: z.number().optional().catch(50),
  streamer_id: z.string().optional(),
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
  const [isCalendarOpen, setIsCalendarOpen] = useState(false);

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
    { value: 'all', label: 'All', icon: Filter },
    { value: 'active', label: 'Live', icon: Activity },
    { value: 'completed', label: 'Done', icon: CheckCircle2 },
  ];

  const timeFilters = [
    { value: 'all', label: 'All Time' },
    { value: 'today', label: 'Today' },
    { value: 'yesterday', label: 'Yesterday' },
    { value: 'week', label: 'Week' },
    { value: 'month', label: 'Month' },
  ];

  const hasActiveFilters =
    search.streamer_id ||
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

  const item = {
    hidden: { opacity: 0, y: 20 },
    show: { opacity: 1, y: 0 },
  };

  return (
    <motion.div
      className="min-h-screen space-y-6"
      variants={container}
      initial="hidden"
      animate="show"
    >
      {/* Header */}
      <motion.div
        className="border-b border-border/40"
        variants={item}
      >
        <div className="w-full">
          <div className="flex flex-col md:flex-row gap-4 items-start md:items-center justify-between p-4 md:px-8">
            <div className="flex items-center gap-4">
              <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
                <Film className="h-6 w-6 text-primary" />
              </div>
              <div>
                <h1 className="text-xl font-semibold tracking-tight">
                  <Trans>Sessions</Trans>
                </h1>
                <p className="text-sm text-muted-foreground">
                  <Trans>Review recorded streams and manage archives</Trans>
                </p>
              </div>
            </div>

            <div className="flex items-center gap-2 w-full md:w-auto overflow-x-auto no-scrollbar">
              {/* Search */}
              <div className="relative w-full md:w-56 min-w-[200px]">
                <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                <Input
                  placeholder={t`Search sessions...`}
                  value={search.streamer_id || ''}
                  onChange={(e) =>
                    updateSearch({
                      streamer_id: e.target.value || undefined,
                      page: 1,
                    })
                  }
                  className="pl-9 h-9 bg-muted/40 border-border/50 focus:bg-background transition-colors"
                />
              </div>

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
                          : 'Custom'}
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
            </div>
          </div>
        </div>
      </motion.div>

      <div className="p-4 md:px-8 space-y-6 pb-20">
        <SessionList
          sessions={query.data?.items || []}
          isLoading={query.isLoading}
          onRefresh={() => query.refetch()}
          token={user?.token?.access_token}
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
