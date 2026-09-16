import { QueryClient, QueryObserver } from '@tanstack/react-query';

import { templatesQueryOptions } from './templates';

const listTemplatesMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions', () => ({
  listTemplates: listTemplatesMock,
}));

/** Mirrors the app's client, whose non-zero default stale time is what an empty
 * `initialData` placeholder would be measured against. */
function createQueryClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 10_000 } },
  });
}

/** Subscribing is what a component mounting the query does. */
function mount(queryClient: QueryClient) {
  const observer = new QueryObserver(queryClient, templatesQueryOptions);
  const unsubscribe = observer.subscribe(() => {});
  return { observer, unsubscribe };
}

describe('templatesQueryOptions', () => {
  beforeEach(() => {
    listTemplatesMock.mockReset();
    listTemplatesMock.mockResolvedValue([{ id: '1', name: 'Default' }]);
  });

  it('fetches on mount so a template picker is never left empty', async () => {
    const queryClient = createQueryClient();

    const { observer, unsubscribe } = mount(queryClient);
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toEqual([
        { id: '1', name: 'Default' },
      ]),
    );
    unsubscribe();

    expect(listTemplatesMock).toHaveBeenCalledTimes(1);
  });

  it('reuses one fetch across the pages that read the list', async () => {
    const queryClient = createQueryClient();

    const first = mount(queryClient);
    await vi.waitFor(() =>
      expect(first.observer.getCurrentResult().isSuccess).toBe(true),
    );
    first.unsubscribe();

    const second = mount(queryClient);
    await vi.waitFor(() =>
      expect(second.observer.getCurrentResult().isSuccess).toBe(true),
    );
    second.unsubscribe();

    expect(listTemplatesMock).toHaveBeenCalledTimes(1);
  });

  it('refetches after a write invalidates the list', async () => {
    const queryClient = createQueryClient();

    const { observer, unsubscribe } = mount(queryClient);
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().isSuccess).toBe(true),
    );
    await queryClient.invalidateQueries({
      queryKey: templatesQueryOptions.queryKey,
    });
    unsubscribe();

    expect(listTemplatesMock).toHaveBeenCalledTimes(2);
  });
});
