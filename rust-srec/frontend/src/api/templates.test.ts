import { QueryObserver, type QueryClient } from '@tanstack/react-query';

import { templatesQueryOptions } from './templates';
import { getContext } from '@/integrations/tanstack-query/root-provider';

const listTemplatesMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions', () => ({
  listTemplates: listTemplatesMock,
}));

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
    const { queryClient } = getContext();

    const { observer, unsubscribe } = mount(queryClient);
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toEqual([
        { id: '1', name: 'Default' },
      ]),
    );
    unsubscribe();

    expect(listTemplatesMock).toHaveBeenCalledTimes(1);
  });
});
