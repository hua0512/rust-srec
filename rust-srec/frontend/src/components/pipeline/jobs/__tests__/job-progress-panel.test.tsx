import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { JobSchema } from '@/api/schemas/pipeline';
import { JobProgressSnapshotSchema } from '@/api/schemas/session';
import { JobProgressPanel } from '../job-progress-panel';

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

afterEach(() => {
  vi.useRealTimers();
});

function renderRunningJob(snapshot: Record<string, unknown>) {
  const now = new Date().toISOString();
  const job = JobSchema.parse({
    id: 'job-1',
    session_id: 'session-1',
    streamer_id: 'streamer-1',
    status: 'PROCESSING',
    processor_type: 'remux',
    input_path: ['/rec/a.flv'],
    created_at: now,
    started_at: now,
  });
  render(
    <I18nProvider i18n={i18n}>
      <JobProgressPanel
        job={job}
        snapshot={JobProgressSnapshotSchema.parse({
          updated_at: now,
          ...snapshot,
        })}
      />
    </I18nProvider>,
  );
}

it('shows FFmpeg progress as media time, speed and time left', () => {
  renderRunningJob({
    percent: 20,
    out_time_ms: 754_000,
    duration_ms: 3_720_000,
    speed_x: 2.3,
    eta_secs: 180,
  });

  expect(
    screen.getByText('00:12:34 / 01:02:00 · 2.3× · ~3m left'),
  ).toBeInTheDocument();
  expect(screen.getByRole('progressbar')).toHaveAttribute(
    'aria-valuenow',
    '20',
  );
});

it('shows byte progress with speed, time left and the file in a batch', () => {
  renderRunningJob({
    percent: 25,
    bytes_done: 256 * 1024 * 1024,
    bytes_total: 1024 * 1024 * 1024,
    speed_bytes_per_sec: 50 * 1024 * 1024,
    eta_secs: 15,
    files_done: 1,
    files_total: 3,
  });

  expect(
    screen.getByText('256 MB / 1 GB · 50 MB/s · ~15s left · File 2 of 3'),
  ).toBeInTheDocument();
  expect(screen.getByText('25%')).toBeInTheDocument();
});

it('shows progress whose phase this client does not know', () => {
  renderRunningJob({ phase: 'verifying', percent: 40 });

  expect(screen.getByText('40%')).toBeInTheDocument();
});

it.each([
  ['plain work', undefined, true],
  ['finalizing', 'finalizing', false],
  ['preparing files to upload', 'preparing', false],
])(
  'flags progress unchanged for a minute only outside finalizing and preparing (%s)',
  (_name, phase, flagged) => {
    vi.useFakeTimers();
    renderRunningJob({ phase, percent: 99, out_time_ms: 60_000 });

    act(() => {
      vi.advanceTimersByTime(61_000);
    });

    expect(screen.queryByText(/^No progress for/) != null).toBe(flagged);
  },
);
