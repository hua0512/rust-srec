import { beforeEach, describe, expect, it, vi } from 'vitest';

const reveal = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/plugin-opener', () => ({
  revealItemInDir: reveal,
}));

import { revealItemInDir } from '../tauri';

describe('revealItemInDir', () => {
  beforeEach(() => {
    reveal.mockReset();
  });

  it('points the file manager at the recording itself', async () => {
    reveal.mockResolvedValue(undefined);

    await revealItemInDir('/output/streamer/recording.mp4');

    expect(reveal).toHaveBeenCalledWith('/output/streamer/recording.mp4');
  });

  // A recording deleted outside the application must surface as an error the
  // caller can show, not as a silently successful no-op.
  it('rejects when the item cannot be revealed', async () => {
    reveal.mockRejectedValue(new Error('No such file or directory'));

    await expect(revealItemInDir('/gone.mp4')).rejects.toThrow(
      'No such file or directory',
    );
  });
});
