import { describe, expect, it } from 'vitest';

import {
  EngineConfigOverrideSchema,
  MesioConfigOverrideSchema,
  MesioConfigSchema,
} from '../engine';

describe('Mesio override resets', () => {
  it.each([
    { buffer_size: null, fix_flv: null, fix_hls: null, fix_dash: null },
    { hls: null, flv_fix: null },
    { hls: { base: null, output_config: { buffer_limits: null } } },
    {
      hls: {
        base: { timeout_ms: null, params: null, http_version: null },
        playlist_config: { variant_selection_policy: null },
        output_config: {
          live_gap_strategy: null,
          vod_segment_timeout_ms: null,
          live_max_overall_stall_duration_ms: null,
          metrics_enabled: null,
        },
      },
    },
    {
      flv_fix: {
        sequence_header_change_mode: null,
        duplicate_tag_filtering: null,
        duplicate_tag_filter_config: { window_capacity_tags: null },
      },
    },
    { flv_fix: { duplicate_tag_filter_config: null } },
  ])('keeps the resets in %j', (override) => {
    expect(MesioConfigOverrideSchema.parse(override)).toEqual(override);
    expect(EngineConfigOverrideSchema.parse(override)).toEqual(override);
  });

  it('still validates the values beside the resets', () => {
    expect(
      MesioConfigOverrideSchema.safeParse({
        hls: { base: null, fetcher_config: { max_segment_retries: -1 } },
      }).success,
    ).toBe(false);
    expect(
      MesioConfigOverrideSchema.safeParse({ flv_fix: { unknown: null } })
        .success,
    ).toBe(false);
  });

  it('refuses a custom value left empty', () => {
    for (const override of [
      { buffer_size: '' },
      { hls: { base: { user_agent: '  ' } } },
      { hls: { fetcher_config: { max_segment_retries: '' } } },
      {
        flv_fix: { duplicate_tag_filter_config: { window_capacity_tags: '' } },
      },
    ]) {
      expect(MesioConfigOverrideSchema.safeParse(override).success).toBe(false);
    }
  });

  it('refuses an emptied DASH value or policy number', () => {
    for (const override of [
      { dash: { max_segment_retries: '' } },
      { dash: { audio_language: '  ' } },
      { dash: { live_start: { type: 'delay', delay_ms: '' } } },
      {
        hls: {
          output_config: {
            live_gap_strategy: { type: 'skip_after_count', count: '' },
          },
        },
      },
      {
        hls: {
          playlist_config: {
            variant_selection_policy: {
              type: 'matching_resolution',
              width: '',
              height: 720,
            },
          },
        },
      },
    ]) {
      expect(MesioConfigOverrideSchema.safeParse(override).success).toBe(false);
    }
    // An engine leaves an emptied field unset, which is the default.
    expect(
      MesioConfigSchema.parse({ dash: { max_segment_retries: '' } }).dash,
    ).toBeUndefined();
    expect(
      MesioConfigSchema.safeParse({
        dash: { live_start: { type: 'delay', delay_ms: '' } },
      }).success,
    ).toBe(false);
  });

  it('keeps settings the form does not show', () => {
    const override = {
      hls: { fetcher_config: { progress_emit_min_bytes: null } },
    };
    expect(MesioConfigOverrideSchema.parse(override)).toEqual(override);
  });

  it('keeps engine settings the form does not show', () => {
    expect(
      MesioConfigSchema.parse({
        hls: { fetcher_config: { progress_emit_min_bytes: 1 } },
      }).hls,
    ).toEqual({ fetcher_config: { progress_emit_min_bytes: 1 } });
  });

  it('leaves the engine settings as they were', () => {
    expect(
      MesioConfigSchema.safeParse({ hls: { base: { timeout_ms: null } } })
        .success,
    ).toBe(false);
    // An engine disables these timeouts with `null`.
    expect(
      MesioConfigSchema.parse({
        hls: { output_config: { vod_segment_timeout_ms: null } },
      }).hls,
    ).toEqual({ output_config: { vod_segment_timeout_ms: null } });
  });
});
