import { setupI18n } from '@lingui/core';
import { expect, it } from 'vitest';

import { JobProgressSnapshotSchema } from '@/api/schemas/session';
import { describeJobProgress, snapshotProgress } from '../job-progress';

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

it('leads with the phase and ends with the file name', () => {
  const snapshot = JobProgressSnapshotSchema.parse({
    updated_at: new Date().toISOString(),
    phase: 'finalizing',
    percent: 66,
    out_time_ms: 3_720_000,
    duration_ms: 3_720_000,
    files_done: 1,
    files_total: 3,
    current_file: 'seg_002.flv',
  });

  expect(describeJobProgress(snapshotProgress(snapshot), i18n).details).toEqual(
    ['Finalizing...', '01:02:00 / 01:02:00', 'File 2 of 3', 'seg_002.flv'],
  );
});
