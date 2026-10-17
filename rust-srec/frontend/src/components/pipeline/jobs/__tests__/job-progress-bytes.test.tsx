import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { JobProgressSnapshotSchema } from '@/api/schemas/session';
import { JobProgressBytes } from '../job-progress-bytes';

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function renderSnapshot(snapshot: unknown) {
  return render(
    <I18nProvider i18n={i18n}>
      <JobProgressBytes snapshot={JobProgressSnapshotSchema.parse(snapshot)} />
    </I18nProvider>,
  );
}

describe('JobProgressBytes', () => {
  it('shows copy progress with its label, bytes and batch position', () => {
    renderSnapshot({
      kind: 'copy',
      updated_at: '2026-10-17T00:00:00Z',
      percent: 25,
      bytes_done: 256 * 1024 * 1024,
      bytes_total: 1024 * 1024 * 1024,
      speed_bytes_per_sec: 50 * 1024 * 1024,
      eta_secs: 15,
      raw: { file_index: 2, file_count: 3, file: '/rec/a.flv' },
    });

    expect(screen.getByText('Copied')).toBeTruthy();
    expect(screen.getByText('25%')).toBeTruthy();
    expect(screen.getByText('256 MB / 1 GB')).toBeTruthy();
    expect(screen.getByText('File 2 of 3')).toBeTruthy();
    expect(screen.getByRole('progressbar').getAttribute('aria-valuenow')).toBe(
      '25',
    );
  });

  it('omits the file position for a single-file job', () => {
    renderSnapshot({
      kind: 'copy',
      updated_at: '2026-10-17T00:00:00Z',
      bytes_done: 0,
      bytes_total: 0,
      raw: { file_index: 1, file_count: 1, file: '/rec/a.flv' },
    });

    expect(screen.getByText('100%')).toBeTruthy();
    expect(screen.queryByText(/^File /)).toBeNull();
  });

  it('renders nothing without byte totals', () => {
    const { container } = renderSnapshot({
      kind: 'ffmpeg',
      updated_at: '2026-10-17T00:00:00Z',
      out_time_ms: 1000,
    });

    expect(container.textContent).toBe('');
  });
});
