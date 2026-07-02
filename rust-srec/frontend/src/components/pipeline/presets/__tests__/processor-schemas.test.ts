import { describe, expect, it } from 'vitest';
import { CopyMoveConfigSchema, RcloneConfigSchema } from '../processor-schemas';

describe('processor time anchor schemas', () => {
  it('defaults missing rclone time_anchor to job_created', () => {
    expect(RcloneConfigSchema.parse({}).time_anchor).toBe('job_created');
    expect(
      RcloneConfigSchema.parse({ destination_root: 'remote:/%Y/%m/%d' })
        .time_anchor,
    ).toBe('job_created');
  });

  it('preserves explicit rclone session_start anchor', () => {
    expect(
      RcloneConfigSchema.parse({ time_anchor: 'session_start' }).time_anchor,
    ).toBe('session_start');
  });

  it('keeps copy_move time_anchor optional for legacy execution-time behavior', () => {
    expect(
      CopyMoveConfigSchema.parse({ destination: '/dest' }).time_anchor,
    ).toBe(undefined);
    expect(
      CopyMoveConfigSchema.parse({
        destination: '/dest',
        time_anchor: 'session_start',
      }).time_anchor,
    ).toBe('session_start');
  });
});
