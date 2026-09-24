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

  it('keys the first page with the defaults the page shows', () => {
    expect(streamersListQueryOptions({}).queryKey).toEqual([
      'streamers',
      1,
      24,
      '',
      'all',
      'all',
      'all',
      'all',
      [],
      'default',
    ]);
  });

  it('keys equal search params identically, so the page finds the loader entry', () => {
    const search = { page: 2, q: 'abc', exceptional: ['FATAL_ERROR'] };
    expect(streamersListQueryOptions(search).queryKey).toEqual(
      streamersListQueryOptions({ ...search }).queryKey,
    );
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
