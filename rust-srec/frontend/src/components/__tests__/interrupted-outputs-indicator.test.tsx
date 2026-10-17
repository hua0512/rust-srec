import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { toast } from 'sonner';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { InterruptedOutputsIndicator } from '../interrupted-outputs-indicator';
import { splitPath } from '../pipeline/interrupted-output-list';
import type { InterruptedOutput } from '@/api/schemas';
import {
  batchDeleteInterruptedOutputs,
  deleteInterruptedOutput,
  listInterruptedOutputs,
} from '@/server/functions/pipeline';

vi.mock('@tanstack/react-router', () => ({
  Link: ({
    children,
    to,
    params,
    className,
  }: {
    children: ReactNode;
    to: string;
    params?: { jobId: string };
    className?: string;
  }) => (
    <a href={to.replace('$jobId', params?.jobId ?? '')} className={className}>
      {children}
    </a>
  ),
}));

vi.mock('@/server/functions/pipeline', () => ({
  listInterruptedOutputs: vi.fn(),
  deleteInterruptedOutput: vi.fn(),
  batchDeleteInterruptedOutputs: vi.fn(),
}));

vi.mock('sonner', () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

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
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nProvider i18n={i18n}>
        <InterruptedOutputsIndicator />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe('InterruptedOutputsIndicator', () => {
  beforeEach(() => {
    vi.mocked(listInterruptedOutputs).mockReset();
    vi.mocked(deleteInterruptedOutput).mockReset();
    vi.mocked(batchDeleteInterruptedOutputs).mockReset();
  });

  it('renders nothing while no partial file is left', async () => {
    const { container } = renderIndicator([]);
    await waitFor(() => expect(listInterruptedOutputs).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('lists each file with its folder, size and job', async () => {
    renderIndicator([
      output('a', '/videos/stream.tmp-1.mp4', 2048),
      output('b', 'D:\\rec\\clip.tmp-2.flv', null),
    ]);

    fireEvent.click(
      await screen.findByRole('button', {
        name: '2 partial files from interrupted jobs (2 KB)',
      }),
    );

    expect(screen.getByText('stream.tmp-1.mp4')).toBeInTheDocument();
    expect(screen.getByText('/videos')).toBeInTheDocument();
    expect(screen.getByText('2 KB')).toBeInTheDocument();
    expect(screen.getByText('clip.tmp-2.flv')).toBeInTheDocument();
    expect(screen.getByText('D:\\rec')).toBeInTheDocument();
    expect(screen.getByText('Folder unavailable')).toBeInTheDocument();
    expect(screen.getAllByText('View job')[0].closest('a')).toHaveAttribute(
      'href',
      '/pipeline/jobs/job-a',
    );
  });

  it('keeps or deletes a single file', async () => {
    vi.mocked(deleteInterruptedOutput).mockResolvedValue({
      id: 'a',
      file_deleted: true,
    });
    renderIndicator([output('a', '/videos/stream.tmp-1.mp4', 10)]);

    fireEvent.click(
      await screen.findByRole('button', {
        name: '1 partial file from interrupted jobs (10 B)',
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
    await waitFor(() =>
      expect(deleteInterruptedOutput).toHaveBeenCalledWith({
        data: { id: 'a', keepFile: true },
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() =>
      expect(deleteInterruptedOutput).toHaveBeenLastCalledWith({
        data: { id: 'a', keepFile: false },
      }),
    );
  });

  it('confirms before deleting every file', async () => {
    vi.mocked(batchDeleteInterruptedOutputs).mockResolvedValue({
      requested: 2,
      succeeded: 2,
      failed: 0,
      results: [],
    });
    renderIndicator([
      output('a', '/videos/a.tmp-1.mp4', 10),
      output('b', '/videos/b.tmp-2.mp4', 10),
    ]);

    fireEvent.click(
      await screen.findByRole('button', {
        name: '2 partial files from interrupted jobs (20 B)',
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Delete all' }));
    expect(batchDeleteInterruptedOutputs).not.toHaveBeenCalled();

    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(
      Array.from(dialog.querySelectorAll('button')).find(
        (button) => button.textContent === 'Delete all',
      )!,
    );
    await waitFor(() =>
      expect(batchDeleteInterruptedOutputs).toHaveBeenCalledWith({
        data: { ids: ['a', 'b'], keep_files: false },
      }),
    );
  });
});

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

describe('splitPath', () => {
  it('splits on either separator', () => {
    expect(splitPath('/a/b/c.mp4')).toEqual({ name: 'c.mp4', folder: '/a/b' });
    expect(splitPath('C:\\x\\y.mp4')).toEqual({
      name: 'y.mp4',
      folder: 'C:\\x',
    });
    expect(splitPath('bare.mp4')).toEqual({ name: 'bare.mp4', folder: '' });
  });
});
