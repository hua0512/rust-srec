import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  keepPreviousData,
  useQueryClient,
} from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import { listSessions, deleteSessions } from '@/server/functions/sessions';
import { SessionList } from '@/components/sessions/session-list';
import { endOfDay, startOfDay, subDays } from 'date-fns';
import { useMemo, useState, useCallback } from 'react';
import {
  Film,
  Filter,
  Activity,
  CheckCircle2,
  CalendarDays,
  X,
  LayoutGrid,
  RefreshCcw,
  SearchX,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg, plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { ListPagination } from '@/components/shared/list-pagination';
import { PageEmptyState } from '@/components/shared/list-panel';
import { SelectModeButton } from '@/components/shared/page-actions';
import { SessionBatchActionBar } from '@/components/sessions/session-batch-action-bar';
import { QueryError } from '@/components/shared/query-error';
import { cn } from '@/lib/utils';
import { FILTER_PILL_GROUP, FilterPill } from '@/components/shared/filter-pill';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Calendar } from '@/components/ui/calendar';
import { Button } from '@/components/ui/button';
import { AnimatePresence } from 'motion/react';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { SearchInput } from '@/components/shared/search-input';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { useBatchSelection } from '@/hooks/use-batch-selection';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { formatDate } from '@/lib/datetime';
import { COMPACT_CARD_GRID } from '@/lib/grid';

const SHORT_DATE: Intl.DateTimeFormatOptions = {
  month: 'short',
  day: '2-digit',
};

export const Route = createLazyFileRoute('/_authed/_dashboard/sessions/')({
  component: SessionsPage,
});

type SessionsSearch = ReturnType<typeof Route.useSearch>;
type SessionStatus = NonNullable<SessionsSearch['status']>;
type SessionTimeRange = NonNullable<SessionsSearch['timeRange']>;

const DEFAULT_LIMIT = 20;

function SessionsPage() {
  const search = Route.useSearch();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();
  const [isCalendarOpen, setIsCalendarOpen] = useState(false);

  // Calculate dates based on timeRange or use custom dates
  const { from_date, to_date } = useMemo(() => {
    if (search.timeRange === 'custom' && search.from) {
      // The picker stores local midnight of each picked day. The range covers
      // the whole end day, and a range with only a start (labelled as that
      // single day) covers just that day.
      return {
        from_date: search.from,
        to_date: endOfDay(new Date(search.to ?? search.from)).toISOString(),
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

  // Sent with the request rather than left to the server function's own
  // default, so the page count below is always computed from the size used.
  const limit = search.limit || DEFAULT_LIMIT;

  const query = useQuery({
    queryKey: queryKeys.sessions.list({
      page: search.page,
      limit,
      streamer_id: search.streamer_id,
      search: search.search,
      active_only: activeOnly,
      from_date,
      to_date,
    }),
    queryFn: () =>
      listSessions({
        data: {
          page: search.page,
          limit,
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

  const updateSearch = useUpdateSearch<typeof search>();

  const currentPage = search.page || 1;

  // The URL counts pages from 1; the pager counts them from 0.
  const handlePageIndexChange = useCallback(
    (index: number) =>
      updateSearch({ page: index > 0 ? index + 1 : undefined }),
    [updateSearch],
  );

  const statusFilters: {
    value: SessionStatus;
    label: string;
    icon: typeof Filter;
  }[] = [
    { value: 'all', label: i18n._(msg`All`), icon: Filter },
    { value: 'active', label: i18n._(msg`Live`), icon: Activity },
    { value: 'completed', label: i18n._(msg`Done`), icon: CheckCircle2 },
  ];

  // `custom` is set from the calendar, not from a pill.
  const timeFilters: {
    value: Exclude<SessionTimeRange, 'custom'>;
    label: string;
  }[] = [
    { value: 'all', label: i18n._(msg`All Time`) },
    { value: 'today', label: i18n._(msg`Today`) },
    { value: 'yesterday', label: i18n._(msg`Yesterday`) },
    { value: 'week', label: i18n._(msg`Week`) },
    { value: 'month', label: i18n._(msg`Month`) },
  ];

  const hasActiveFilters = Boolean(
    search.streamer_id ||
    search.search ||
    (search.status && search.status !== 'all') ||
    (search.timeRange && search.timeRange !== 'all') ||
    search.from,
  );
  const clearFilters = () =>
    updateSearch({
      streamer_id: undefined,
      search: undefined,
      status: undefined,
      timeRange: undefined,
      from: undefined,
      to: undefined,
      page: undefined,
    });

  const currentStatus = search.status || 'all';
  const currentTimeRange = search.timeRange || 'all';
  const dateRange = {
    from: search.from ? new Date(search.from) : undefined,
    to: search.to ? new Date(search.to) : undefined,
  };

  const pageIds = useMemo(
    () => (query.data?.items || []).map((session) => session.id),
    [query.data?.items],
  );

  const {
    selectionMode,
    selectedIds,
    allPageSelected,
    handleSelectionChange,
    selectPage,
    clearSelection,
    toggleSelectionMode,
    exitSelectionMode,
  } = useBatchSelection({
    pageIds,
    scope: [
      currentPage,
      limit,
      search.streamer_id ?? '',
      search.search ?? '',
      currentStatus,
      currentTimeRange,
      search.from ?? '',
      search.to ?? '',
    ].join('|'),
  });

  const batchDeleteMutation = useMutation({
    mutationFn: (ids: string[]) => deleteSessions({ data: ids }),
    onSuccess: (result) => {
      toast.success(
        i18n._(
          msg`${plural(result.deleted, {
            one: 'Deleted # session',
            other: 'Deleted # sessions',
          })}`,
        ),
      );
      exitSelectionMode();
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions.all });
    },
    onError: (error) => {
      console.error('Failed to delete sessions:', error);
      toast.error(i18n._(msg`Failed to delete sessions`));
    },
  });

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
                updateSearch({ search: val || undefined, page: undefined })
              }
              placeholder={i18n._(msg`Search sessions...`)}
              className="md:w-56 min-w-[200px]"
            />

            <div className="hidden sm:block h-6 w-px bg-border/50 mx-1 shrink-0" />

            {/* Status Pills */}
            <div className={FILTER_PILL_GROUP}>
              {statusFilters.map((filter) => (
                <FilterPill
                  key={filter.value}
                  icon={filter.icon}
                  isActive={currentStatus === filter.value}
                  onClick={() =>
                    updateSearch({ status: filter.value, page: undefined })
                  }
                >
                  {filter.label}
                </FilterPill>
              ))}
            </div>

            {/* Time Range: wider than a phone, so it scrolls rather than widening the page. */}
            <div
              className={cn(
                FILTER_PILL_GROUP,
                'max-w-full overflow-x-auto no-scrollbar [&>*]:shrink-0',
              )}
            >
              <Popover open={isCalendarOpen} onOpenChange={setIsCalendarOpen}>
                {/* A popover trigger reports its state through aria-expanded
                    rather than aria-pressed. */}
                <PopoverTrigger asChild>
                  <FilterPill
                    icon={CalendarDays}
                    aria-pressed={undefined}
                    isActive={
                      currentTimeRange === 'custom' || Boolean(dateRange.from)
                    }
                  >
                    {dateRange.from
                      ? dateRange.to
                        ? `${formatDate(i18n.locale, dateRange.from, SHORT_DATE)} - ${formatDate(i18n.locale, dateRange.to, SHORT_DATE)}`
                        : formatDate(i18n.locale, dateRange.from, SHORT_DATE)
                      : i18n._(msg`Custom`)}
                  </FilterPill>
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
                          page: undefined,
                        });
                      } else {
                        updateSearch({
                          timeRange: 'all',
                          from: undefined,
                          to: undefined,
                          page: undefined,
                        });
                      }
                    }}
                    disabled={(date) =>
                      date > new Date() || date < new Date('1900-01-01')
                    }
                    numberOfMonths={2}
                    autoFocus
                    className="rounded-md border shadow-xs"
                  />
                </PopoverContent>
              </Popover>

              {timeFilters.map((filter) => (
                <FilterPill
                  key={filter.value}
                  isActive={currentTimeRange === filter.value}
                  onClick={() =>
                    updateSearch({
                      timeRange: filter.value,
                      from: undefined,
                      to: undefined,
                      page: undefined,
                    })
                  }
                >
                  {filter.label}
                </FilterPill>
              ))}
            </div>

            {hasActiveFilters && (
              <Button
                variant="ghost"
                size="icon"
                onClick={clearFilters}
                aria-label={i18n._(msg`Clear filters`)}
                title={i18n._(msg`Clear filters`)}
                className="h-8 w-8 rounded-full hover:bg-destructive/10 hover:text-destructive shrink-0"
              >
                <X className="h-4 w-4" />
              </Button>
            )}

            <SelectModeButton
              selectionMode={selectionMode}
              onToggle={toggleSelectionMode}
              label={i18n._(msg`Select sessions`)}
            />
          </>
        }
      />

      <div className="p-4 md:px-8 space-y-6 pb-20">
        {query.isError && !query.data ? (
          <QueryError
            title={<Trans>Failed to load sessions</Trans>}
            error={query.error}
            onRetry={() => void query.refetch()}
          />
        ) : query.isPending ? (
          <div className={cn(COMPACT_CARD_GRID, 'gap-4')}>
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
        ) : query.data?.items && query.data.items.length > 0 ? (
          <SessionList
            sessions={query.data.items}
            selectionMode={selectionMode}
            selectedIds={selectedIds}
            onSelectionChange={handleSelectionChange}
          />
        ) : hasActiveFilters ? (
          <PageEmptyState
            icon={SearchX}
            title={<Trans>No sessions found</Trans>}
            help={
              <Trans>No sessions match the current search or filters.</Trans>
            }
            action={
              <Button variant="outline" onClick={clearFilters}>
                <X className="mr-2 h-4 w-4" />
                <Trans>Clear filters</Trans>
              </Button>
            }
          />
        ) : (
          <PageEmptyState
            icon={LayoutGrid}
            title={<Trans>No Archives Found</Trans>}
            help={
              <Trans>
                Your digital library is currently empty. Start recording to
                populate your sessions here.
              </Trans>
            }
            action={
              <Button variant="outline" onClick={() => void query.refetch()}>
                <RefreshCcw className="mr-2 h-4 w-4" />
                <Trans>Refresh</Trans>
              </Button>
            }
          />
        )}

        <ListPagination
          pageIndex={currentPage - 1}
          totalCount={query.data?.total}
          pageSize={limit}
          onPageIndexChange={handlePageIndexChange}
        />
      </div>

      <AnimatePresence>
        {selectionMode && (
          <SessionBatchActionBar
            selectedCount={selectedIds.size}
            pageCount={pageIds.length}
            allPageSelected={allPageSelected}
            isPending={batchDeleteMutation.isPending}
            onSelectPage={selectPage}
            onClearSelection={clearSelection}
            onDelete={() => batchDeleteMutation.mutate(Array.from(selectedIds))}
            onExit={toggleSelectionMode}
          />
        )}
      </AnimatePresence>
    </div>
  );
}
