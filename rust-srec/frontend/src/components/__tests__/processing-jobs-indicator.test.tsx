import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';

import { ProcessingJobsIndicator } from '../processing-jobs-indicator';
import { useUploadStore, type JobView } from '@/store/uploads';

vi.mock('@tanstack/react-router', () => ({
  Link: ({
    children,
    to,
    params,
  }: {
    children: ReactNode;
    to: string;
    params?: { jobId: string };
  }) => <a href={to.replace('$jobId', params?.jobId ?? '')}>{children}</a>,
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function createJob(overrides: Partial<JobView>): JobView {
  return {
    jobId: 'job-1',
    streamerId: 'streamer-1',
    streamerName: 'Streamer One',
    streamerAvatar: '',
    jobType: 'remux',
    startedAtMs: BigInt(Date.now() - 60_000),
    lastEventAtMs: Date.now(),
    ...overrides,
  };
}

it('lists each running job with its step and progress, linked to the job', () => {
  const jobs = [
    createJob({
      percent: 20,
      outTimeMs: 754_000n,
      durationMs: 3_720_000n,
      speedX: 2.3,
      etaSecs: 180,
    }),
    createJob({
      jobId: 'job-2',
      streamerName: 'Streamer Two',
      jobType: 'execute',
    }),
  ];
  useUploadStore.setState({
    jobsByJobId: new Map(jobs.map((job) => [job.jobId, job])),
  });
  render(
    <I18nProvider i18n={i18n}>
      <ProcessingJobsIndicator />
    </I18nProvider>,
  );

  fireEvent.click(screen.getByRole('button', { name: '2 jobs in progress' }));

  expect(screen.getByText('Streamer One').closest('a')).toHaveAttribute(
    'href',
    '/pipeline/jobs/job-1',
  );
  expect(
    screen.getByText('Remux / Transcode').nextElementSibling,
  ).toHaveTextContent('00:12:34 / 01:02:00 · 2.3× · ~3m left');
  expect(screen.getByRole('progressbar')).toHaveAttribute(
    'aria-valuenow',
    '20',
  );
  // Without progress reports a job shows how long it has run, and no bar.
  expect(
    screen.getByText('Execute Command').nextElementSibling,
  ).toHaveTextContent(/^Running for 1m/);
});
