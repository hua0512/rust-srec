import { QueryClient } from '@tanstack/react-query';

import { UploadTerminalStatus } from '@/api/proto/gen/download_progress_pb.js';
import { handleUploadTerminal } from '../WebSocketProvider';

function terminal(status: UploadTerminalStatus) {
  return {
    jobId: 'job-1',
    streamerId: 'streamer-1',
    status,
    error: status === UploadTerminalStatus.FAILED ? 'quota exceeded' : '',
    filesSucceeded: 1,
    filesFailed: status === UploadTerminalStatus.FAILED ? 2 : 0,
  };
}

describe('handleUploadTerminal', () => {
  it('removes the live upload and invalidates only its durable records', async () => {
    const queryClient = new QueryClient();
    const targetKey = ['pipeline', 'job', 'job-1', 'uploads'] as const;
    const otherKey = ['pipeline', 'job', 'job-2', 'uploads'] as const;
    queryClient.setQueryData(targetKey, { items: [] });
    queryClient.setQueryData(otherKey, { items: [] });
    const removeUpload = vi.fn();
    const failUpload = vi.fn();

    await handleUploadTerminal(
      queryClient,
      terminal(UploadTerminalStatus.COMPLETED),
      { removeUpload, failUpload },
    );

    expect(removeUpload).toHaveBeenCalledExactlyOnceWith('job-1');
    expect(failUpload).not.toHaveBeenCalled();
    expect(queryClient.getQueryState(targetKey)?.isInvalidated).toBe(true);
    expect(queryClient.getQueryState(otherKey)?.isInvalidated).toBe(false);
  });

  it('keeps a failed upload, with its error, instead of removing it', async () => {
    const removeUpload = vi.fn();
    const failUpload = vi.fn();

    await handleUploadTerminal(
      new QueryClient(),
      terminal(UploadTerminalStatus.FAILED),
      { removeUpload, failUpload },
    );

    expect(removeUpload).not.toHaveBeenCalled();
    expect(failUpload).toHaveBeenCalledExactlyOnceWith({
      jobId: 'job-1',
      streamerId: 'streamer-1',
      error: 'quota exceeded',
      filesSucceeded: 1,
      filesFailed: 2,
    });
  });
});
