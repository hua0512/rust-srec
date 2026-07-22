import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
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
  listTemplates,
  batchUpdateStreamers,
} from '@/server/functions';
import type { BatchStreamerAction } from '@/api/schemas';

import { Button } from '@/components/ui/button';
import {
  Plus,
  Users,
  Video,
  Wifi,
  WifiOff,
  AlertTriangle,
  Ban,
  Radio,
  LayoutTemplate,
  ListFilter,
  RotateCcw,
  ListChecks,
} from 'lucide-react';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { StreamerCard } from '@/components/streamers/streamer-card';
import { SearchInput } from '@/components/shared/search-input';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { containerVariants, itemVariants } from '@/lib/animation';
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
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Checkbox } from '@/components/ui/checkbox';
import { Label } from '@/components/ui/label';
import { StreamerBatchActionBar } from '@/components/streamers/streamer-batch-action-bar';

export const Route = createLazyFileRoute('/_authed/_dashboard/streamers/')({
  component: StreamersPage,
});

const PAGE_SIZES = [12, 24, 48, 96];

type PriorityFilter = 'all' | 'HIGH' | 'NORMAL' | 'LOW';
type SortOption =
  | 'default'
  | 'name-asc'
  | 'name-desc'
  | 'priority-desc'
  | 'priority-asc'
  | 'state-asc'
  | 'updated-desc';

function StreamersPage() {
  const { i18n } = useLingui();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // State filters defined inside the component to ensure they are re-translated when locale changes
  const STATE_FILTERS = [
    { value: 'all', label: i18n._(msg`All`), icon: Users },
    { value: 'LIVE', label: i18n._(msg`Live`), icon: Wifi },
    { value: 'NOT_LIVE', label: i18n._(msg`Offline`), icon: WifiOff },
    { value: 'ERROR', label: i18n._(msg`Error`), icon: AlertTriangle },
    { value: 'DISABLED', label: i18n._(msg`Disabled`), icon: Ban },
  ];

  const search = Route.useSearch();
  const updateSearch = useUpdateSearch<typeof search>();

  // Filters/search/pagination live in the URL so they survive navigation into a
  // streamer detail/edit page and reloads. Selection state stays local.
  const page = search.page ?? 1;
  const pageSize = search.size ?? 24;
  const debouncedSearch = search.q ?? '';
  const platformFilter = search.platform ?? 'all';
  const templateFilter = search.template ?? 'all';
  const stateFilter = search.state ?? 'all';
  const priorityFilter: PriorityFilter = search.priority ?? 'all';
  const exceptionalStates = search.exceptional ?? [];
  const sortOption: SortOption = search.sort ?? 'default';
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  const exceptionalStateOptions = [
    { value: 'OUT_OF_SPACE', label: i18n._(msg`Out of space`) },
    { value: 'FATAL_ERROR', label: i18n._(msg`Fatal error`) },
    { value: 'NOT_FOUND', label: i18n._(msg`Not found`) },
    {
      value: 'TEMPORAL_DISABLED',
      label: i18n._(msg`Temporarily disabled`),
    },
  ];

  const activeSecondaryFilterCount =
    Number(priorityFilter !== 'all') +
    Number(exceptionalStates.length > 0) +
    Number(sortOption !== 'default');

  // Handlers — selecting a primary state clears exceptional-state checkboxes and
  // vice versa (the two feed the same `state` query arg); every filter change
  // resets to the first page via `page: undefined`.
  const handleStateChange = useCallback(
    (value: string) => {
      updateSearch({
        state: value === 'all' ? undefined : value,
        exceptional: undefined,
        page: undefined,
      });
    },
    [updateSearch],
  );

  const handlePlatformChange = useCallback(
    (value: string) => {
      updateSearch({
        platform: value === 'all' ? undefined : value,
        page: undefined,
      });
    },
    [updateSearch],
  );

  const handleTemplateChange = useCallback(
    (value: string) => {
      updateSearch({
        template: value === 'all' ? undefined : value,
        page: undefined,
      });
    },
    [updateSearch],
  );

  const handleExceptionalStateChange = useCallback(
    (value: string, checked: boolean) => {
      const next = checked
        ? exceptionalStates.includes(value)
          ? exceptionalStates
          : [...exceptionalStates, value]
        : exceptionalStates.filter((state) => state !== value);
      updateSearch({
        exceptional: next.length > 0 ? next : undefined,
        state: undefined,
        page: undefined,
      });
    },
    [exceptionalStates, updateSearch],
  );

  const clearSecondaryFilters = useCallback(() => {
    updateSearch({
      priority: undefined,
      exceptional: undefined,
      sort: undefined,
      page: undefined,
    });
  }, [updateSearch]);

  // Fetch Platforms
  const { data: platforms = [] } = useQuery({
    queryKey: ['platforms'],
    queryFn: () => listPlatformConfigs(),
    staleTime: 60000,
  });

  const { data: templates = [] } = useQuery({
    queryKey: ['templates'],
    queryFn: () => listTemplates(),
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
      templateFilter,
      stateFilter,
      priorityFilter,
      exceptionalStates,
      sortOption,
    ],
    queryFn: async () => {
      const platform = platformFilter === 'all' ? undefined : platformFilter;
      const template =
        templateFilter === 'all' || templateFilter === '__unassigned__'
          ? undefined
          : templateFilter;
      const state =
        exceptionalStates.length > 0
          ? exceptionalStates.join(',')
          : stateFilter === 'all'
            ? undefined
            : stateFilter;
      const priority = priorityFilter === 'all' ? undefined : priorityFilter;
      const [sortBy, sortDir] =
        sortOption === 'default'
          ? [undefined, undefined]
          : (sortOption.split('-') as [
              'name' | 'priority' | 'state' | 'updated',
              'asc' | 'desc',
            ]);
      return listStreamers({
        data: {
          page,
          limit: pageSize,
          search: debouncedSearch,
          platform,
          template,
          templateUnassigned: templateFilter === '__unassigned__',
          state,
          priority,
          sortBy: sortBy === 'updated' ? 'updated_at' : sortBy,
          sortDir,
        },
      });
    },
    placeholderData: keepPreviousData,
    refetchInterval: 5000,
  });

  const streamers = streamersData?.items || [];
  const totalCount = streamersData?.total || 0;
  const totalPages = Math.ceil(totalCount / pageSize);
  const allPageSelected =
    streamers.length > 0 &&
    streamers.every((streamer) => selectedIds.has(streamer.id));

  const selectionScope = [
    page,
    pageSize,
    debouncedSearch,
    platformFilter,
    templateFilter,
    stateFilter,
    priorityFilter,
    exceptionalStates.join(','),
    sortOption,
  ].join('|');

  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectionScope]);

  // Page overflow protection: reset to last valid page when filters reduce results
  useEffect(() => {
    if (page > totalPages && totalPages > 0) {
      updateSearch({ page: totalPages });
    }
  }, [totalPages, page, updateSearch]);

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
      toast.success(i18n._(msg`Streamer deleted`));
      void queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) =>
      toast.error(error.message || i18n._(msg`Failed to delete streamer`)),
  });

  const checkMutation = useMutation({
    mutationFn: (id: string) => checkStreamer({ data: id }),
    onSuccess: () => toast.success(i18n._(msg`Check triggered`)),
    onError: (error: any) =>
      toast.error(error.message || i18n._(msg`Failed to trigger check`)),
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      updateStreamer({ data: { id, data: { enabled } } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Streamer updated`));
      void queryClient.invalidateQueries({ queryKey: ['streamers'] });
    },
    onError: (error: any) =>
      toast.error(error.message || i18n._(msg`Failed to update streamer`)),
  });

  const batchMutation = useMutation({
    mutationFn: ({
      ids,
      action,
    }: {
      ids: string[];
      action: BatchStreamerAction;
    }) => batchUpdateStreamers({ data: { ids, action } }),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: ['streamers'] });

      if (result.failed === 0) {
        toast.success(
          i18n._(msg`Successfully processed ${result.succeeded} streamers`),
        );
        setSelectedIds(new Set());
        setSelectionMode(false);
        return;
      }

      const failedResults = result.results.filter((item) => !item.success);
      setSelectedIds(new Set(failedResults.map((item) => item.id)));
      toast.warning(
        i18n._(
          msg`Processed ${result.succeeded} streamers; ${result.failed} failed`,
        ),
        {
          description: failedResults
            .slice(0, 3)
            .map((item) => item.error)
            .filter(Boolean)
            .join('; '),
        },
      );
    },
    onError: (error) => {
      toast.error(
        error instanceof Error
          ? error.message
          : i18n._(msg`Failed to process selected streamers`),
      );
    },
  });

  const handleSelectionChange = useCallback((id: string, selected: boolean) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (selected) {
        next.add(id);
      } else {
        next.delete(id);
      }
      return next;
    });
  }, []);

  const toggleSelectionMode = useCallback(() => {
    setSelectionMode((current) => {
      if (current) setSelectedIds(new Set());
      return !current;
    });
  }, []);

  const handleBatchAction = useCallback(
    (action: BatchStreamerAction) => {
      if (selectedIds.size === 0 || batchMutation.isPending) return;
      batchMutation.mutate({ ids: Array.from(selectedIds), action });
    },
    [selectedIds, batchMutation],
  );

  const handleDelete = (id: string) => {
    if (confirm(i18n._(msg`Are you sure you want to delete this streamer?`))) {
      deleteMutation.mutate(id);
    }
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
    <div className="min-h-screen space-y-6">
      {/* Header */}
      <DashboardHeader
        icon={Video}
        title={<Trans>Streamers</Trans>}
        subtitle={<Trans>Manage your monitored channels and downloads</Trans>}
        actions={
          <>
            <SearchInput
              defaultValue={debouncedSearch}
              onSearch={(value) =>
                updateSearch({ q: value || undefined, page: undefined })
              }
              placeholder={i18n._(msg`Search streamers...`)}
              className="flex-1 md:w-56 min-w-[200px]"
            />

            <Badge
              variant="secondary"
              className="h-9 px-3 text-sm whitespace-nowrap"
            >
              {totalCount} <Trans>total</Trans>
            </Badge>
          </>
        }
      >
        <div className="flex w-full flex-col gap-1 rounded-2xl border border-border/60 bg-muted/30 p-1 shadow-xs sm:inline-flex sm:w-auto sm:flex-row sm:items-center sm:gap-0.5 sm:rounded-full">
          <nav className="grid w-full grid-cols-5 gap-0.5 sm:flex sm:w-auto sm:items-center">
            {STATE_FILTERS.map((filter) => {
              const Icon = filter.icon;
              const isActive =
                exceptionalStates.length === 0 && stateFilter === filter.value;
              return (
                <button
                  key={filter.value}
                  onClick={() => handleStateChange(filter.value)}
                  aria-pressed={isActive}
                  className={`flex h-8 min-w-0 items-center justify-center gap-2 rounded-full px-1.5 text-xs font-medium transition-colors sm:px-3 sm:text-sm ${
                    isActive
                      ? 'bg-background text-primary shadow-xs ring-1 ring-border/50'
                      : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'
                  }`}
                >
                  <Icon className="hidden h-3.5 w-3.5 shrink-0 sm:block" />
                  <span className="truncate">{filter.label}</span>
                </button>
              );
            })}
          </nav>

          <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto_auto_auto] gap-1 sm:contents">
            <Select value={platformFilter} onValueChange={handlePlatformChange}>
              <SelectTrigger
                className={`h-8 w-full rounded-full border-0 px-2.5 shadow-none transition-colors hover:bg-background/60 focus-visible:ring-2 sm:w-40 ${
                  platformFilter === 'all'
                    ? 'text-muted-foreground dark:bg-transparent'
                    : 'bg-background text-foreground shadow-xs dark:bg-background'
                }`}
              >
                <div className="flex min-w-0 items-center gap-2">
                  <Radio
                    className={`h-3.5 w-3.5 shrink-0 ${
                      platformFilter === 'all'
                        ? 'text-muted-foreground'
                        : 'text-primary'
                    }`}
                  />
                  <span className="truncate">
                    <SelectValue placeholder={i18n._(msg`Platform`)} />
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

            <Select value={templateFilter} onValueChange={handleTemplateChange}>
              <SelectTrigger
                className={`h-8 w-full rounded-full border-0 px-2.5 shadow-none transition-colors hover:bg-background/60 focus-visible:ring-2 sm:w-40 ${
                  templateFilter === 'all'
                    ? 'text-muted-foreground dark:bg-transparent'
                    : 'bg-background text-foreground shadow-xs dark:bg-background'
                }`}
              >
                <div className="flex min-w-0 items-center gap-2">
                  <LayoutTemplate
                    className={`h-3.5 w-3.5 shrink-0 ${
                      templateFilter === 'all'
                        ? 'text-muted-foreground'
                        : 'text-primary'
                    }`}
                  />
                  <span className="truncate">
                    <SelectValue placeholder={i18n._(msg`Template`)} />
                  </span>
                </div>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  <Trans>All Templates</Trans>
                </SelectItem>
                <SelectItem value="__unassigned__">
                  <Trans>No template assigned</Trans>
                </SelectItem>
                {templates.map((template) => (
                  <SelectItem key={template.id} value={template.id}>
                    {template.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <Popover>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label={i18n._(msg`More filters`)}
                  className={`h-8 rounded-full px-2.5 ${
                    activeSecondaryFilterCount > 0
                      ? 'bg-background text-primary shadow-xs ring-1 ring-border/50'
                      : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'
                  }`}
                >
                  <ListFilter className="h-3.5 w-3.5" />
                  <span className="hidden md:inline">
                    <Trans>Filters</Trans>
                  </span>
                  {activeSecondaryFilterCount > 0 && (
                    <span className="flex h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold text-primary-foreground">
                      {activeSecondaryFilterCount}
                    </span>
                  )}
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="end"
                className="w-[calc(100vw-2rem)] rounded-2xl p-0 sm:w-80"
              >
                <div className="flex items-center justify-between border-b px-4 py-3">
                  <div>
                    <p className="font-semibold">
                      <Trans>More filters</Trans>
                    </p>
                    <p className="text-xs text-muted-foreground">
                      <Trans>Refine and sort streamers</Trans>
                    </p>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={activeSecondaryFilterCount === 0}
                    onClick={clearSecondaryFilters}
                    className="rounded-full text-muted-foreground"
                  >
                    <RotateCcw className="h-3.5 w-3.5" />
                    <Trans>Clear</Trans>
                  </Button>
                </div>

                <div className="space-y-5 p-4">
                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-2">
                      <Label>
                        <Trans>Priority</Trans>
                      </Label>
                      <Select
                        value={priorityFilter}
                        onValueChange={(value) => {
                          updateSearch({
                            priority:
                              value === 'all'
                                ? undefined
                                : (value as Exclude<PriorityFilter, 'all'>),
                            page: undefined,
                          });
                        }}
                      >
                        <SelectTrigger className="w-full rounded-xl">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="all">
                            <Trans>Any priority</Trans>
                          </SelectItem>
                          <SelectItem value="HIGH">
                            <Trans>High</Trans>
                          </SelectItem>
                          <SelectItem value="NORMAL">
                            <Trans>Normal</Trans>
                          </SelectItem>
                          <SelectItem value="LOW">
                            <Trans>Low</Trans>
                          </SelectItem>
                        </SelectContent>
                      </Select>
                    </div>

                    <div className="space-y-2">
                      <Label>
                        <Trans>Sort by</Trans>
                      </Label>
                      <Select
                        value={sortOption}
                        onValueChange={(value) => {
                          updateSearch({
                            sort:
                              value === 'default'
                                ? undefined
                                : (value as Exclude<SortOption, 'default'>),
                            page: undefined,
                          });
                        }}
                      >
                        <SelectTrigger className="w-full rounded-xl">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="default">
                            <Trans>Default</Trans>
                          </SelectItem>
                          <SelectItem value="updated-desc">
                            <Trans>Recently updated</Trans>
                          </SelectItem>
                          <SelectItem value="name-asc">
                            <Trans>Name A-Z</Trans>
                          </SelectItem>
                          <SelectItem value="name-desc">
                            <Trans>Name Z-A</Trans>
                          </SelectItem>
                          <SelectItem value="priority-desc">
                            <Trans>Priority high-low</Trans>
                          </SelectItem>
                          <SelectItem value="priority-asc">
                            <Trans>Priority low-high</Trans>
                          </SelectItem>
                          <SelectItem value="state-asc">
                            <Trans>State</Trans>
                          </SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>

                  <div className="space-y-3 border-t pt-4">
                    <Label>
                      <Trans>Exceptional states</Trans>
                    </Label>
                    <div className="grid grid-cols-2 gap-3">
                      {exceptionalStateOptions.map((option) => {
                        const checked = exceptionalStates.includes(
                          option.value,
                        );
                        const id = `exception-state-${option.value}`;
                        return (
                          <Label
                            key={option.value}
                            htmlFor={id}
                            className="cursor-pointer font-normal"
                          >
                            <Checkbox
                              id={id}
                              checked={checked}
                              onCheckedChange={(nextChecked) =>
                                handleExceptionalStateChange(
                                  option.value,
                                  nextChecked === true,
                                )
                              }
                            />
                            <span className="truncate">{option.label}</span>
                          </Label>
                        );
                      })}
                    </div>
                  </div>
                </div>
              </PopoverContent>
            </Popover>

            <div
              className="h-5 w-px shrink-0 self-center bg-border"
              aria-hidden="true"
            />

            <Button
              variant="ghost"
              size="sm"
              onClick={toggleSelectionMode}
              aria-pressed={selectionMode}
              aria-label={i18n._(msg`Select streamers`)}
              className={`h-8 rounded-full px-2.5 ${
                selectionMode
                  ? 'bg-background text-primary shadow-xs ring-1 ring-border/50'
                  : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'
              }`}
            >
              <ListChecks className="h-3.5 w-3.5" />
              <span className="hidden lg:inline">
                <Trans>Select</Trans>
              </span>
            </Button>
          </div>
        </div>
      </DashboardHeader>

      {/* Content Content */}
      <div className="p-4 md:px-8 pb-20">
        <AnimatePresence mode="wait">
          {isLoading ? (
            <motion.div
              key="loading"
              initial={false}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {[1, 2, 3, 4, 5, 6, 7, 8].map((i) => (
                <div
                  key={i}
                  className="border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 overflow-hidden"
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
              variants={containerVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              {streamers.map((streamer) => (
                <motion.div key={streamer.id} variants={itemVariants}>
                  <StreamerCard
                    streamer={streamer}
                    selectionMode={selectionMode}
                    isSelected={selectedIds.has(streamer.id)}
                    onSelectionChange={handleSelectionChange}
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
                  updateSearch({ size: Number(v), page: undefined });
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
              aria-label={i18n._(msg`Streamer pagination`)}
            >
              <PaginationContent>
                <PaginationItem>
                  <PaginationPrevious
                    onClick={() => page > 1 && updateSearch({ page: page - 1 })}
                    aria-label={i18n._(msg`Go to previous page`)}
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
                        onClick={() => updateSearch({ page: p })}
                        aria-label={i18n._(msg`Go to page ${p}`)}
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
                    onClick={() =>
                      page < totalPages && updateSearch({ page: page + 1 })
                    }
                    aria-label={i18n._(msg`Go to next page`)}
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

        <AnimatePresence>
          {!selectionMode && (
            <motion.div
              className="fixed bottom-8 right-8 z-50"
              initial={{ scale: 0.93, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: 0.93, opacity: 0 }}
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
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence>
        {selectionMode && (
          <StreamerBatchActionBar
            selectedCount={selectedIds.size}
            pageCount={streamers.length}
            allPageSelected={allPageSelected}
            templates={templates}
            isPending={batchMutation.isPending}
            onSelectPage={() =>
              setSelectedIds(new Set(streamers.map((streamer) => streamer.id)))
            }
            onClearSelection={() => setSelectedIds(new Set())}
            onAction={handleBatchAction}
            onExit={toggleSelectionMode}
          />
        )}
      </AnimatePresence>
    </div>
  );
}
