import { screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';

import { DagStepSchema } from '@/api/schemas/pipeline';
import { DagGraphView } from '../dag-graph-view';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

it('shows running, queued and retrying steps, with a bar only for a reported percent', () => {
  const now = Date.now();
  const step = (id: string, fields: Record<string, unknown>) =>
    DagStepSchema.parse({
      step_id: id,
      status: 'PROCESSING',
      job_id: `job-${id}`,
      depends_on: [],
      outputs: [],
      processor: id,
      ...fields,
    });
  const steps = [
    step('remux', {
      job_status: 'PROCESSING',
      started_at: now,
      progress: { updated_at: new Date(now).toISOString(), percent: 42 },
    }),
    step('thumbnail', { job_status: 'PENDING' }),
    step('rclone', { job_status: 'FAILED', retry_after: now + 60_000 }),
  ];

  renderWithProviders(<DagGraphView steps={steps} />);

  expect(screen.getByText('Running')).toBeInTheDocument();
  expect(screen.getByText('Queued')).toBeInTheDocument();
  expect(screen.getByText('Retrying')).toBeInTheDocument();
  const bars = screen.getAllByRole('progressbar');
  expect(bars).toHaveLength(1);
  expect(bars[0]).toHaveAttribute('aria-valuenow', '42');
});

it('titles each node by its step id, so steps sharing a processor differ', () => {
  const step = (id: string) =>
    DagStepSchema.parse({
      step_id: id,
      status: 'PENDING',
      depends_on: [],
      outputs: [],
      processor: 'execute',
    });

  renderWithProviders(
    <DagGraphView steps={[step('notify_discord'), step('archive__notify')]} />,
  );

  expect(screen.getByText('notify discord')).toBeInTheDocument();
  expect(screen.getByText('archive › notify')).toBeInTheDocument();
});
