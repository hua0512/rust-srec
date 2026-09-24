import { queryOptions } from '@tanstack/react-query';
import { listStreamers } from '@/server/functions/streamers';
import type { Priority } from '@/api/schemas/common';

export const STREAMERS_DEFAULT_PAGE_SIZE = 24;

export type StreamersSortOption =
  | 'name-asc'
  | 'name-desc'
  | 'priority-desc'
  | 'priority-asc'
  | 'state-asc'
  | 'updated-desc';

/** The streamers page's URL search params that select which page is listed. */
export interface StreamersListSearch {
  page?: number;
  size?: number;
  q?: string;
  platform?: string;
  template?: string;
  state?: string;
  priority?: Priority;
  exceptional?: string[];
  sort?: StreamersSortOption;
}

/**
 * One page of the streamers list for the given URL search params. The route
 * loader and the page read it through this factory so that the page finds the
 * entry the loader filled, on the server and on hover preloads alike.
 */
export function streamersListQueryOptions(search: StreamersListSearch) {
  const page = search.page ?? 1;
  const pageSize = search.size ?? STREAMERS_DEFAULT_PAGE_SIZE;
  const searchText = search.q ?? '';
  const platformFilter = search.platform ?? 'all';
  const templateFilter = search.template ?? 'all';
  const stateFilter = search.state ?? 'all';
  const priorityFilter = search.priority ?? 'all';
  const exceptionalStates = search.exceptional ?? [];
  const sortOption = search.sort ?? 'default';

  return queryOptions({
    queryKey: [
      'streamers',
      page,
      pageSize,
      searchText,
      platformFilter,
      templateFilter,
      stateFilter,
      priorityFilter,
      exceptionalStates,
      sortOption,
    ],
    queryFn: () => {
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
          search: searchText,
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
  });
}
