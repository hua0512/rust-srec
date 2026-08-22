import { QueryClient } from '@tanstack/react-query';

import { handleUploadTerminal } from '../WebSocketProvider';

describe('handleUploadTerminal', () => {
  it('removes the live upload and invalidates only its durable records', async () => {
    const queryClient = new QueryClient();
    const targetKey = ['pipeline', 'job', 'job-1', 'uploads'] as const;
    const otherKey = ['pipeline', 'job', 'job-2', 'uploads'] as const;
    queryClient.setQueryData(targetKey, { items: [] });
    queryClient.setQueryData(otherKey, { items: [] });
    const removeUpload = vi.fn();

    await handleUploadTerminal(queryClient, 'job-1', removeUpload);

    expect(removeUpload).toHaveBeenCalledExactlyOnceWith('job-1');
    expect(queryClient.getQueryState(targetKey)?.isInvalidated).toBe(true);
    expect(queryClient.getQueryState(otherKey)?.isInvalidated).toBe(false);
  });
});
