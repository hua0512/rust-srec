import { streamersListQueryOptions } from './streamers';

const listStreamersMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions/streamers', () => ({
  listStreamers: listStreamersMock,
}));

describe('streamersListQueryOptions', () => {
  beforeEach(() => {
    listStreamersMock.mockReset();
    listStreamersMock.mockResolvedValue({
      items: [],
      total: 0,
      limit: 24,
      offset: 0,
    });
  });

  it('turns the search params into the backend filters', async () => {
    const { queryFn } = streamersListQueryOptions({
      page: 3,
      size: 48,
      q: 'abc',
      platform: 'huya',
      template: '__unassigned__',
      exceptional: ['FATAL_ERROR', 'NOT_FOUND'],
      state: 'LIVE',
      priority: 'HIGH',
      sort: 'updated-desc',
    });

    await (queryFn as () => Promise<unknown>)();

    expect(listStreamersMock).toHaveBeenCalledWith({
      data: {
        page: 3,
        limit: 48,
        search: 'abc',
        platform: 'huya',
        template: undefined,
        templateUnassigned: true,
        state: 'FATAL_ERROR,NOT_FOUND',
        priority: 'HIGH',
        sortBy: 'updated_at',
        sortDir: 'desc',
      },
    });
  });
});
