import { describe, expect, it } from 'vitest';
import { ExecuteConfigSchema } from '../processor-schemas';

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
});
