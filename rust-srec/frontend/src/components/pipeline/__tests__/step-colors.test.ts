import { describe, expect, it } from 'vitest';
import { VALID_CATEGORIES, VALID_PROCESSORS } from '@/api/schemas';
import {
  DEFAULT_STEP_COLOR,
  STEP_COLORS,
  getStepBadgeColor,
  getStepColor,
} from '../constants';

/**
 * Tailwind only emits utilities it finds verbatim in the source, so a class
 * assembled at render time (`bg-${color}/10`) silently renders untinted.
 */
const STATIC_CLASS_LIST = /^[a-z0-9/-]+(?: [a-z0-9/-]+)*$/;

const COLOR_ENTRIES = Object.entries(STEP_COLORS);

const ALL_VARIANTS: [string, string][] = [
  ...COLOR_ENTRIES.flatMap(([token, variants]): [string, string][] => [
    [`${token} gradient`, variants.gradient],
    [`${token} badge`, variants.badge],
  ]),
  ['default gradient', DEFAULT_STEP_COLOR.gradient],
  ['default badge', DEFAULT_STEP_COLOR.badge],
];

/** Every preset name shipped by the migrations; workflow steps store these verbatim. */
const BUILT_IN_PRESET_NAMES = [
  'remux',
  'remux_mkv',
  'remux_faststart',
  'remux_clean',
  'compress_fast',
  'compress_hq',
  'compress_archive',
  'compress_hevc_max',
  'compress_ultrafast',
  'thumbnail',
  'thumbnail_hd',
  'thumbnail_fullhd',
  'thumbnail_max',
  'thumbnail_native',
  'thumbnail_preview',
  'audio_mp3',
  'audio_mp3_hq',
  'audio_aac',
  'archive_zip',
  'delete_source',
  'copy',
  'move',
  'upload',
  'upload_and_delete',
  'baidupcs_upload',
  'add_metadata',
  'execute',
  'custom_ffmpeg',
  'danmu_to_ass',
  'ass_burnin',
  'nvenc_h264_fast',
  'nvenc_h264_hq',
  'nvenc_h264_lowlatency',
  'nvenc_hevc_fast',
  'nvenc_hevc_hq',
  'nvenc_av1_fast',
  'nvenc_av1_hq',
];

describe('step colours', () => {
  it.each(ALL_VARIANTS)('%s is a complete static class list', (_, value) => {
    expect(value).toMatch(STATIC_CLASS_LIST);
  });

  it.each(COLOR_ENTRIES)(
    '%s badge carries background, text and border tints',
    (_token, variants) => {
      expect(variants.badge).toMatch(/(?:^| )bg-[a-z]+-\d+\/10(?: |$)/);
      expect(variants.badge).toMatch(/(?:^| )text-[a-z]+-\d+(?: |$)/);
      expect(variants.badge).toMatch(/(?:^| )border-[a-z]+-\d+\/20(?: |$)/);
    },
  );

  it.each([...VALID_PROCESSORS])(
    'resolves a tint for the %s processor',
    (name) => {
      expect(getStepBadgeColor(name)).not.toBe(DEFAULT_STEP_COLOR.badge);
      expect(getStepColor(name)).not.toBe(DEFAULT_STEP_COLOR.gradient);
    },
  );

  it.each([...VALID_CATEGORIES])(
    'resolves a tint for the %s category',
    (name) => {
      expect(getStepBadgeColor('unmapped_processor', name)).not.toBe(
        DEFAULT_STEP_COLOR.badge,
      );
    },
  );

  it.each(BUILT_IN_PRESET_NAMES)('resolves a tint for preset %s', (name) => {
    expect(getStepBadgeColor(name)).not.toBe(DEFAULT_STEP_COLOR.badge);
  });

  it('falls back to the neutral tint for unknown steps', () => {
    expect(getStepBadgeColor('totally_unknown')).toBe(DEFAULT_STEP_COLOR.badge);
    expect(getStepColor('totally_unknown')).toBe(DEFAULT_STEP_COLOR.gradient);
  });
});
