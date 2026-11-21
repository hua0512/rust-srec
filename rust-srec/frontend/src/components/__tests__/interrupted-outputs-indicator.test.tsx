import { fireEvent, screen, waitFor } from '@testing-library/react';
import { toast } from 'sonner';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { InterruptedOutputsIndicator } from '../interrupted-outputs-indicator';
import type { InterruptedOutput } from '@/api/schemas';
import {
  batchDeleteInterruptedOutputs,
  listInterruptedOutputs,
} from '@/server/functions/pipeline';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/pipeline', () => ({
  listInterruptedOutputs: vi.fn(),
  batchDeleteInterruptedOutputs: vi.fn(),
}));

vi.mock('sonner', () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

function output(
  id: string,
  path: string,
  size: number | null,
): InterruptedOutput {
  return {
    id,
    path,
    output_path: path.replace(/\.tmp-[^.]+/, ''),
    job_id: `job-${id}`,
    created_at: new Date(Date.now() - 120_000).toISOString(),
    interrupted_at: new Date(Date.now() - 60_000).toISOString(),
    size_bytes: size,
    exists: size == null ? null : true,
  };
}

function renderIndicator(outputs: InterruptedOutput[]) {
  vi.mocked(listInterruptedOutputs).mockResolvedValue({
    outputs,
    total_size_bytes: outputs.reduce((sum, o) => sum + (o.size_bytes ?? 0), 0),
  });
  return renderWithProviders(<InterruptedOutputsIndicator />);
}

describe('deleting every file', () => {
  beforeEach(() => {
    vi.mocked(batchDeleteInterruptedOutputs).mockReset();
    vi.mocked(toast.success).mockReset();
  });

  it('sends more files than one batch accepts in several requests', async () => {
    vi.mocked(batchDeleteInterruptedOutputs).mockImplementation(
      async ({ data }) => ({
        requested: data.ids.length,
        succeeded: data.ids.length,
        failed: 0,
        results: data.ids.map((id) => ({ id, success: true })),
      }),
    );
    const outputs = Array.from({ length: 150 }, (_, index) =>
      output(String(index), `/videos/${index}.tmp-${index}.mp4`, 1),
    );
    renderIndicator(outputs);

    fireEvent.click(
      await screen.findByRole('button', {
        name: '150 partial files from interrupted jobs (150 B)',
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Delete all' }));
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(
      Array.from(dialog.querySelectorAll('button')).find(
        (button) => button.textContent === 'Delete all',
      )!,
    );

    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith('Deleted 150 partial files'),
    );
    const batches = vi
      .mocked(batchDeleteInterruptedOutputs)
      .mock.calls.map(([{ data }]) => data.ids);
    expect(batches.map((ids) => ids.length)).toEqual([100, 50]);
    expect(batches.flat()).toEqual(outputs.map((item) => item.id));
  });
});
