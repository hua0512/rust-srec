import { StreamlinkConfigOverrideSchema } from '../engine';
import {
  CreateTemplateRequestSchema,
  UpdateTemplateRequestSchema,
} from '../template';

describe('Streamlink FFmpeg path overrides', () => {
  it.each([undefined, null, '', '   ', '/opt/media tools/ffmpeg'])(
    'preserves %s through template create/update validation',
    (ffmpegPath) => {
      const override =
        ffmpegPath === undefined ? {} : { ffmpeg_path: ffmpegPath };
      for (const schema of [
        CreateTemplateRequestSchema,
        UpdateTemplateRequestSchema,
      ]) {
        const request = schema.parse({
          name: 'Template',
          engines_override: { 'streamlink-1': override },
        });
        const serialized = JSON.parse(JSON.stringify(request));
        expect(serialized.engines_override['streamlink-1']).toEqual(override);
      }
    },
  );

  it('accepts the new field while rejecting unknown override keys', () => {
    expect(
      StreamlinkConfigOverrideSchema.safeParse({ ffmpeg_path: 'ffmpeg-custom' })
        .success,
    ).toBe(true);
    expect(
      StreamlinkConfigOverrideSchema.safeParse({
        ffmpeg_path: 'ffmpeg-custom',
        ffmpeg_pat: 'typo',
      }).success,
    ).toBe(false);
    expect(
      StreamlinkConfigOverrideSchema.safeParse({ ffmpeg_path: 42 }).success,
    ).toBe(false);
  });
});
