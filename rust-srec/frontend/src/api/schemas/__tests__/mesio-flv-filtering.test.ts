import { describe, expect, it } from 'vitest';

import { MesioConfigOverrideSchema, MesioConfigSchema } from '../engine';

describe('Mesio FLV filtering configuration', () => {
  it('keeps missing configuration optional and defaults an empty form to disabled', () => {
    expect(MesioConfigSchema.parse({})).not.toHaveProperty('flv_fix');
    for (const flv_fix of [{}, { duplicate_tag_filter_config: {} }]) {
      const parsed = MesioConfigSchema.parse({ flv_fix });
      expect(parsed.flv_fix).toMatchObject({
        duplicate_tag_filtering: false,
        duplicate_tag_filter_config: {
          enable_replay_offset_matching: false,
          window_capacity_bytes: 16777216,
        },
      });
    }
  });

  it.each([false, true])(
    'preserves explicit filtering choices (%s) when saving',
    (enabled) => {
      const config = {
        flv_fix: {
          duplicate_tag_filtering: enabled,
          duplicate_tag_filter_config: {
            enable_replay_offset_matching: enabled,
            window_capacity_bytes: 1048576,
          },
        },
      };
      const saved = JSON.parse(JSON.stringify(MesioConfigSchema.parse(config)));
      expect(MesioConfigSchema.parse(saved)).toMatchObject(config);
      expect(MesioConfigOverrideSchema.parse(config)).toEqual(config);
    },
  );

  it('keeps partial overrides partial and validates the retained-payload budget', () => {
    const partial = {
      flv_fix: { duplicate_tag_filter_config: { window_capacity_bytes: 0 } },
    };
    expect(MesioConfigOverrideSchema.parse(partial)).toEqual(partial);
    expect(
      MesioConfigSchema.safeParse({
        flv_fix: { duplicate_tag_filter_config: { window_capacity_bytes: -1 } },
      }).success,
    ).toBe(false);
  });
});
