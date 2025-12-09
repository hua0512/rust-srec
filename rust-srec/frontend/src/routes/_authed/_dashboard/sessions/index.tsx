import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { z } from 'zod';
import { listSessions } from '@/server/functions/sessions';
import { SessionFilters } from '@/components/sessions/SessionFilters';
import { SessionList } from '@/components/sessions/SessionList';
import { startOfDay, subDays } from 'date-fns';
import { useMemo } from 'react';

const searchSchema = z.object({
  page: z.number().optional().catch(1),
  limit: z.number().optional().catch(50),
  streamer_id: z.string().optional(),
  status: z.enum(['all', 'active', 'completed']).optional().catch('all'),
  timeRange: z.enum(['all', 'today', 'yesterday', 'week', 'month', 'custom']).optional().catch('all'),
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

  // Calculate dates based on timeRange or use custom dates
  const { from_date, to_date } = useMemo(() => {
    if (search.timeRange === 'custom' && search.from) {
      return {
        from_date: search.from,
        to_date: search.to
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
    search.status === 'active' ? true : search.status === 'completed' ? false : undefined;

  const query = useQuery({
    queryKey: ['sessions', search.page, search.limit, search.streamer_id, activeOnly, from_date, to_date],
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
  });

  const updateSearch = (newParams: Partial<typeof search>) => {
    navigate({
      search: (prev) => ({ ...prev, ...newParams }),
      replace: true,
    });
  };

  return (
    <div className="p-6 space-y-6 max-w-[1600px] w-full">
      <SessionFilters
        search={search.streamer_id || ''}
        onSearchChange={(val) => updateSearch({ streamer_id: val || undefined, page: 1 })}
        status={search.status || 'all'}
        onStatusChange={(val) => updateSearch({ status: val as any, page: 1 })}
        timeRange={search.timeRange || 'all'}
        onTimeRangeChange={(val) => {
          // If switching to a preset (not custom), clear custom dates
          if (val !== 'custom') {
            updateSearch({ timeRange: val as any, from: undefined, to: undefined, page: 1 });
          } else {
            updateSearch({ timeRange: val as any, page: 1 });
          }
        }}
        dateRange={{ from: search.from ? new Date(search.from) : undefined, to: search.to ? new Date(search.to) : undefined }}
        onDateRangeChange={(range) => {
          if (range?.from) {
            updateSearch({
              timeRange: 'custom',
              from: range.from.toISOString(),
              to: range.to?.toISOString(),
              page: 1
            });
          } else {
            updateSearch({ timeRange: 'all', from: undefined, to: undefined, page: 1 });
          }
        }}
        onClear={() => navigate({ search: {}, replace: true })}
      />

      <SessionList
        sessions={query.data?.items || []}
        isLoading={query.isLoading}
        onRefresh={() => query.refetch()}
      />

      {/* Pagination Controls could go here */}
      <div className="flex justify-center gap-2 mt-8">
        <button
          disabled={search.page === 1}
          onClick={() => updateSearch({ page: (search.page || 1) - 1 })}
          className="px-4 py-2 text-sm bg-secondary rounded disabled:opacity-50"
        >
          Previous
        </button>
        <span className="py-2 text-sm text-muted-foreground">Page {search.page || 1}</span>
        <button
          disabled={query.data && query.data.items.length < (search.limit || 50)}
          onClick={() => updateSearch({ page: (search.page || 1) + 1 })}
          className="px-4 py-2 text-sm bg-secondary rounded disabled:opacity-50"
        >
          Next
        </button>
      </div>
    </div>
  );
}

