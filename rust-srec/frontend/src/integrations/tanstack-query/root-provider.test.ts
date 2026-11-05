import { getContext } from './root-provider';

describe('query client defaults', () => {
  it('reuses a recent result instead of fetching again on the next mount', async () => {
    const { queryClient } = getContext();
    const queryFn = vi.fn().mockResolvedValue('value');

    await queryClient.fetchQuery({ queryKey: ['thing'], queryFn });
    await queryClient.fetchQuery({ queryKey: ['thing'], queryFn });

    expect(queryFn).toHaveBeenCalledOnce();
  });
});
