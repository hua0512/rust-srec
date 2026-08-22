import { describe, expect, it } from 'vitest';
import {
  BaiduPcsConfigSchema,
  CopyMoveConfigSchema,
  RcloneConfigSchema,
} from '../processor-schemas';

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

describe('baidupcs config schema', () => {
  it('applies defaults matching the backend BaiduPcsConfig::default', () => {
    const config = BaiduPcsConfigSchema.parse({});
    expect(config.policy).toBe('skip');
    expect(config.time_anchor).toBe('job_created');
    expect(config.norapid).toBe(false);
    expect(config.max_retries).toBe(3);
    expect(config.args).toEqual([]);
    expect(config.remove_source_after_upload).toBe(false);
  });

  it('rejects unknown policies and out-of-range retries', () => {
    expect(() => BaiduPcsConfigSchema.parse({ policy: 'replace' })).toThrow();
    expect(() => BaiduPcsConfigSchema.parse({ max_retries: 0 })).toThrow();
    expect(() => BaiduPcsConfigSchema.parse({ max_retries: 11 })).toThrow();
  });

  it('accepts a full config round-trip', () => {
    const config = BaiduPcsConfigSchema.parse({
      destination_root: '/rust-srec/{streamer}/%Y-%m',
      policy: 'rsync',
      time_anchor: 'session_start',
      norapid: true,
      max_retries: 5,
      args: ['--verbose'],
      remove_source_after_upload: true,
      binary_path: '/usr/local/bin/BaiduPCS-Go',
      config_dir: '/app/config/BaiduPCS-Go',
    });
    expect(config.policy).toBe('rsync');
    expect(config.max_retries).toBe(5);
  });
});
