import { describe, expect, it } from 'vitest';

import { MesioConfigOverrideSchema, MesioConfigSchema } from '../engine';

describe('Mesio FLV filtering configuration', () => {
  it('saves an untouched form without FLV settings, leaving the defaults to the engine', () => {
    expect(MesioConfigSchema.parse({})).not.toHaveProperty('flv_fix');
    for (const flv_fix of [
      {},
      { duplicate_tag_filter_config: {} },
      { duplicate_tag_filtering: undefined, sequence_header_change_mode: null },
    ]) {
      expect(MesioConfigSchema.parse({ flv_fix })).not.toHaveProperty(
        'flv_fix',
      );
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

  it('keeps partial overrides partial', () => {
    const partial = {
      flv_fix: { duplicate_tag_filter_config: { window_capacity_bytes: 0 } },
    };
    expect(MesioConfigOverrideSchema.parse(partial)).toEqual(partial);
  });

  it.each([
    ['full config', MesioConfigSchema],
    ['override', MesioConfigOverrideSchema],
  ] as const)(
    'rejects a negative or fractional payload budget in the %s schema',
    (_, schema) => {
      for (const window_capacity_bytes of [-1, 1.5]) {
        const config = {
          flv_fix: { duplicate_tag_filter_config: { window_capacity_bytes } },
        };
        expect(schema.safeParse(config).success).toBe(false);
      }
    },
  );
});
