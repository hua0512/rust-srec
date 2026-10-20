import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { expect, it, vi } from 'vitest';

import { DagStepSchema } from '@/api/schemas/pipeline';
import { DagGraphView } from '../dag-graph-view';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: ReactNode }) => <a>{children}</a>,
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

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

  render(
    <I18nProvider i18n={i18n}>
      <DagGraphView steps={steps} />
    </I18nProvider>,
  );

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

  render(
    <I18nProvider i18n={i18n}>
      <DagGraphView steps={[step('notify_discord'), step('archive__notify')]} />
    </I18nProvider>,
  );

  expect(screen.getByText('notify discord')).toBeInTheDocument();
  expect(screen.getByText('archive › notify')).toBeInTheDocument();
});
