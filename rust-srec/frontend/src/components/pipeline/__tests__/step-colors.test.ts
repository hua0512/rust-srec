import { describe, expect, it } from 'vitest';
import { VALID_CATEGORIES } from '@/api/schemas';
import {
  DEFAULT_STEP_COLOR,
  PROCESSOR_META,
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

  it.each(Object.keys(PROCESSOR_META))(
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

  // Workflow steps store preset names, which start with their family.
  it.each(['compress_hq', 'nvenc_av1_fast'])(
    'resolves a tint for preset %s',
    (name) => {
      expect(getStepBadgeColor(name)).not.toBe(DEFAULT_STEP_COLOR.badge);
    },
  );

  it('falls back to the neutral tint for unknown steps', () => {
    expect(getStepBadgeColor('totally_unknown')).toBe(DEFAULT_STEP_COLOR.badge);
    expect(getStepColor('totally_unknown')).toBe(DEFAULT_STEP_COLOR.gradient);
  });
});
