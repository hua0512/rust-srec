import { describe, expect, it } from 'vitest';
import {
  BaiduPcsConfigSchema,
  CopyMoveConfigSchema,
  DeleteConfigSchema,
  ExecuteConfigSchema,
  RcloneConfigSchema,
  ThumbnailConfigSchema,
} from '../processor-schemas';

describe('execute config schema', () => {
  it('preserves legacy commands and nullable output scanning', () => {
    const config = {
      command: 'ffmpeg -i "{input}" -c copy "{output}"',
      scan_output_dir: null,
      scan_extension: null,
    };
    expect(ExecuteConfigSchema.parse(config)).toStrictEqual(config);
  });

  it('preserves program paths and each literal argument without shell parsing', () => {
    const config = {
      program: ' C:\\Tools\\custom program.exe ',
      args: ['', '  ', 'a b', '"quoted"', 'first\nsecond', '{input}'],
      scan_output_dir: '/output/processed',
      scan_extension: 'mp4',
    };
    expect(ExecuteConfigSchema.parse(config)).toStrictEqual(config);
    expect(ExecuteConfigSchema.parse({ program: 'ffmpeg' })).toStrictEqual({
      program: 'ffmpeg',
    });
  });

  it('treats null optional mode fields as absent without adding inactive defaults', () => {
    expect(
      ExecuteConfigSchema.parse({
        command: 'ffmpeg -version',
        program: null,
        args: null,
      }),
    ).toStrictEqual({ command: 'ffmpeg -version' });
    expect(
      ExecuteConfigSchema.parse({
        command: null,
        program: 'ffmpeg',
        args: null,
      }),
    ).toStrictEqual({ program: 'ffmpeg' });
  });

  it.each([
    {},
    { command: '' },
    { program: '' },
    { program: ' \t\n' },
    { args: ['-version'] },
    { command: 'ffmpeg -version', program: 'ffmpeg' },
    { command: 'ffmpeg -version', args: [] },
    { command: 'ffmpeg -version', args: ['-version'] },
    { program: 'ffmpeg', args: '-version' },
  ])('rejects invalid mode configurations: %j', (config) => {
    expect(ExecuteConfigSchema.safeParse(config).success).toBe(false);
  });

  it('addresses invalid arguments by index for both form consumers', () => {
    const parsed = ExecuteConfigSchema.safeParse({
      program: 'ffmpeg',
      args: ['-i', 42],
    });
    expect(parsed.success).toBe(false);
    if (!parsed.success) {
      expect(parsed.error.issues[0].path).toEqual(['args', 1]);
    }
  });
});

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

// Counts and pixel sizes are whole numbers on the backend, so a typed decimal
// has to be reported by the form rather than sent on and rejected there.
describe('whole-number processor settings', () => {
  it('rejects a fractional retry count and delay', () => {
    expect(() => DeleteConfigSchema.parse({ max_retries: 2.5 })).toThrow();
    expect(() => DeleteConfigSchema.parse({ retry_delay_ms: 100.5 })).toThrow();
    expect(DeleteConfigSchema.parse({ max_retries: 2 }).max_retries).toBe(2);
  });

  it('rejects a fractional thumbnail width and quality', () => {
    expect(() => ThumbnailConfigSchema.parse({ width: 320.5 })).toThrow();
    expect(() => ThumbnailConfigSchema.parse({ quality: 2.5 })).toThrow();
  });

  it('rejects a fractional rclone retry count while keeping fractional limits', () => {
    expect(() => RcloneConfigSchema.parse({ max_retries: 2.5 })).toThrow();
    expect(RcloneConfigSchema.parse({ tpslimit: 0.5 }).tpslimit).toBe(0.5);
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
