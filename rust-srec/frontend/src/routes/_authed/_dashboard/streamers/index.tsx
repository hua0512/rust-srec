import { createFileRoute, useNavigate } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { useState, useMemo, useEffect, useCallback } from 'react';
import {
  listStreamers,
  deleteStreamer,
  checkStreamer,
  updateStreamer,
  listPlatformConfigs,
} from '@/server/functions';

import { Button } from '@/components/ui/button';
import {
  Plus,
  Search,
  Users,
  Video,
  Wifi,
  WifiOff,
  AlertTriangle,
  Ban,
  Radio,
} from 'lucide-react';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { StreamerCard } from '@/components/streamers/streamer-card';
import { Skeleton } from '@/components/ui/skeleton';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination';

export const Route = createFileRoute('/_authed/_dashboard/streamers/')({
  component: StreamersPage,
});

const PAGE_SIZES = [12, 24, 48, 96];

function StreamersPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // State filters defined inside the component to ensure they are re-translated when locale changes
  const STATE_FILTERS = [
    { value: 'all', label: t`All`, icon: Users },
    { value: 'LIVE', label: t`Live`, icon: Wifi },
    { value: 'NOT_LIVE', label: t`Offline`, icon: WifiOff },
    { value: 'ERROR', label: t`Error`, icon: AlertTriangle },
    { value: 'DISABLED', label: t`Disabled`, icon: Ban },
  ];

  // State
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(24);
  const [search, setSearch] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const [platformFilter, setPlatformFilter] = useState('all');
  const [stateFilter, setStateFilter] = useState('all');

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(search);
      setPage(1);
    }, 300);
    return () => clearTimeout(timer);
  }, [search]);

  // Handlers
  const handleStateChange = useCallback((value: string) => {
    setStateFilter(value);
    setPage(1);
  }, []);

  const handlePlatformChange = useCallback((value: string) => {
    setPlatformFilter(value);
    setPage(1);
  }, []);

  // Fetch Platforms
  const { data: platforms = [] } = useQuery({
    queryKey: ['platforms'],
    queryFn: () => listPlatformConfigs(),
    staleTime: 60000,
  });

  // Fetch Streamers
  const {
    data: streamersData,
    isLoading,
    isError,
    error,
  } = useQuery({
    queryKey: [
      'streamers',
      page,
      pageSize,
      debouncedSearch,
      platformFilter,
      stateFilter,
    ],
    queryFn: async () => {
      const platform = platformFilter === 'all' ? undefined : platformFilter;
      const state = stateFilter === 'all' ? undefined : stateFilter;
      return listStreamers({
        data: {
          page,
          limit: pageSize,
          search: debouncedSearch,
          platform,
          state,
        },
      });
    },
    placeholderData: keepPreviousData,
    refetchInterval: 5000,
  });

  const streamers = streamersData?.items || [];
  const totalCount = streamersData?.total || 0;
  const totalPages = Math.ceil(totalCount / pageSize);

  // Page overflow protection: reset to last valid page when filters reduce results
  useEffect(() => {
    if (page > totalPages && totalPages > 0) {
      setPage(totalPages);
    }
  }, [totalPages, page]);

  // Pagination logic
  const paginationPages = useMemo(() => {
    const pages: (number | 'ellipsis')[] = [];
    // Current interface uses 1-based page, but internal calc might be easier with 0-based conceptual
    const current = page;
    const total = totalPages;

    if (total <= 7) {
      for (let i = 1; i <= total; i++) pages.push(i);
    } else {
      pages.push(1);
      if (current > 3) pages.push('ellipsis');
      for (
        let i = Math.max(2, current - 1);
        i <= Math.min(total - 1, current + 1);
        i++
      ) {
        pages.push(i);
      }
      if (current < total - 2) pages.push('ellipsis');
      pages.push(total);
    }
    return pages;
  }, [totalPages, page]);

  // Mutations
  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteStreamer({ data: id }),
    onSuccess: () => {
      toast.success(t`Streamer deleted`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) =>
      toast.error(error.message || t`Failed to delete streamer`),
  });

  const checkMutation = useMutation({
    mutationFn: (id: string) => checkStreamer({ data: id }),
    onSuccess: () => toast.success(t`Check triggered`),
    onError: (error: any) =>
      toast.error(error.message || t`Failed to trigger check`),
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      updateStreamer({ data: { id, data: { enabled } } }),
    onSuccess: () => {
      toast.success(t`Streamer updated`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) =>
      toast.error(error.message || t`Failed to update streamer`),
  });

  const handleDelete = (id: string) => {
    if (confirm(t`Are you sure you want to delete this streamer?`)) {
      deleteMutation.mutate(id);
    }
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

  if (isError) {
    return (
      <div className="p-8 text-center text-destructive">
        <p>
          <Trans>Error loading streamers: {(error as any).message}</Trans>
        </p>
      </div>
    );
  }

  return (
    <motion.div
      className="min-h-screen space-y-6"
      variants={container}
      initial="hidden"
      animate="show"
    >
      {/* Header */}
      <DashboardHeader
        icon={Video}
        title={<Trans>Streamers</Trans>}
        subtitle={<Trans>Manage your monitored channels and downloads</Trans>}
        actions={
          <>
            {/* Search */}
            <div className="relative flex-1 md:w-56 min-w-[200px]">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
              <Input
                placeholder={t`Search streamers...`}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className="pl-9 h-9"
              />
            </div>

            {/* Platform Select */}
            <Select value={platformFilter} onValueChange={handlePlatformChange}>
              <SelectTrigger className="w-[200px] h-9 bg-background/50 border-input/60 hover:bg-accent/50 transition-colors">
                <div className="flex items-center gap-2 truncate">
                  <Radio className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                  <span className="truncate">
                    <SelectValue placeholder={t`Platform`} />
                  </span>
                </div>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  <Trans>All Platforms</Trans>
                </SelectItem>
                {platforms.map((p) => (
                  <SelectItem key={p.id} value={p.id}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <Badge
              variant="secondary"
              className="h-9 px-3 text-sm whitespace-nowrap"
            >
              {totalCount} <Trans>total</Trans>
            </Badge>
          </>
        }
      >
        <nav className="flex items-center gap-1">
          {STATE_FILTERS.map((filter) => {
            const Icon = filter.icon;
            const isActive = stateFilter === filter.value;
            return (
              <button
                key={filter.value}
                onClick={() => handleStateChange(filter.value)}
                className={`relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 flex items-center gap-2 ${
                  isActive
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted'
                }`}
              >
                <Icon className="h-3.5 w-3.5" />
                <span className="relative z-10">{filter.label}</span>
              </button>
            );
          })}
        </nav>
      </DashboardHeader>

      {/* Content Content */}
      <div className="p-4 md:px-8 pb-20">
        <AnimatePresence mode="wait">
          {isLoading ? (
            <motion.div
              key="loading"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {[1, 2, 3, 4, 5, 6, 7, 8].map((i) => (
                <div
                  key={i}
                  className="h-[200px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4"
                >
                  <div className="flex items-center gap-3">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <div className="space-y-1 flex-1">
                      <Skeleton className="h-4 w-3/4" />
                      <Skeleton className="h-3 w-1/2" />
                    </div>
                  </div>
                  <Skeleton className="h-24 w-full rounded-md" />
                </div>
              ))}
            </motion.div>
          ) : streamers.length > 0 ? (
            <motion.div
              key="list"
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.2 }}
            >
              {streamers.map((streamer, index) => (
                <motion.div
                  key={streamer.id}
                  initial={{ opacity: 0, y: 20 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{
                    delay: index * 0.05,
                  }}
                >
                  <StreamerCard
                    streamer={streamer}
                    onDelete={handleDelete}
                    onToggle={(id, enabled) =>
                      toggleMutation.mutate({ id, enabled })
                    }
                    onCheck={(id) => checkMutation.mutate(id)}
                  />
                </motion.div>
              ))}
            </motion.div>
          ) : (
            <motion.div
              key="empty"
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              className="flex flex-col items-center justify-center py-24 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5"
            >
              <div className="p-6 bg-muted/10 rounded-full ring-1 ring-border/50">
                <Users className="h-12 w-12 text-muted-foreground/50" />
              </div>
              <div className="space-y-2">
                <h3 className="font-semibold text-xl">
                  <Trans>No streamers found</Trans>
                </h3>
                <p className="text-muted-foreground max-w-sm mx-auto">
                  <Trans>
                    Try adjusting your search or filters, or add a new streamer
                    to start monitoring.
                  </Trans>
                </p>
              </div>
              <Button
                onClick={() => navigate({ to: '/streamers/new' })}
                size="lg"
              >
                <Plus className="mr-2 h-5 w-5" />
                <Trans>Add Streamer</Trans>
              </Button>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Pagination */}
        {totalPages > 1 && (
          <div className="flex items-center justify-between mt-8 pt-6 border-t font-medium text-sm">
            <div className="flex items-center gap-3 text-muted-foreground">
              <span>
                <Trans>Rows per page</Trans>
              </span>
              <Select
                value={pageSize.toString()}
                onValueChange={(v) => {
                  setPageSize(Number(v));
                  setPage(1);
                }}
              >
                <SelectTrigger className="w-16 h-8">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PAGE_SIZES.map((size) => (
                    <SelectItem key={size} value={size.toString()}>
                      {size}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <Pagination
              className="w-auto mx-0"
              aria-label={t`Streamer pagination`}
            >
              <PaginationContent>
                <PaginationItem>
                  <PaginationPrevious
                    onClick={() => page > 1 && setPage((p) => p - 1)}
                    aria-label={t`Go to previous page`}
                    aria-disabled={page === 1}
                    tabIndex={page === 1 ? -1 : 0}
                    className={
                      page === 1
                        ? 'pointer-events-none opacity-50 cursor-not-allowed'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>
                {paginationPages.map((p, i) =>
                  p === 'ellipsis' ? (
                    <PaginationItem key={`ellipsis-${i}`} aria-hidden="true">
                      <PaginationEllipsis />
                    </PaginationItem>
                  ) : (
                    <PaginationItem key={p}>
                      <PaginationLink
                        isActive={page === p}
                        onClick={() => setPage(p)}
                        aria-label={t`Go to page ${p}`}
                        aria-current={page === p ? 'page' : undefined}
                        className="cursor-pointer"
                      >
                        {p}
                      </PaginationLink>
                    </PaginationItem>
                  ),
                )}
                <PaginationItem>
                  <PaginationNext
                    onClick={() => page < totalPages && setPage((p) => p + 1)}
                    aria-label={t`Go to next page`}
                    aria-disabled={page === totalPages}
                    tabIndex={page === totalPages ? -1 : 0}
                    className={
                      page === totalPages
                        ? 'pointer-events-none opacity-50 cursor-not-allowed'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>
              </PaginationContent>
            </Pagination>
          </div>
        )}

        {/* FAB */}
        <motion.div
          className="fixed bottom-8 right-8 z-50"
          initial={{ scale: 0.93, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          whileHover={{ scale: 1.1 }}
          whileTap={{ scale: 0.9 }}
        >
          <Button
            onClick={() => navigate({ to: '/streamers/new' })}
            size="icon"
            className="h-14 w-14 rounded-full shadow-2xl bg-primary hover:bg-primary/90 text-primary-foreground flex items-center justify-center p-0"
          >
            <Plus className="h-6 w-6" />
          </Button>
        </motion.div>
      </div>
    </motion.div>
  );
}
