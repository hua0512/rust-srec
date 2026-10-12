import { describe, expect, it } from 'vitest';

import {
  dashOverrideDisablesBothTracks,
  EngineConfigOverrideSchema,
  MesioConfigOverrideSchema,
  MesioConfigSchema,
  resolveDashSettings,
} from '../engine';
import { overridesWithoutDashTracks } from '@/components/config/templates/engine-override-checks';

const full = {
  video_quality: { type: 'max_height', height: 720 },
  audio_quality: { type: 'closest_bandwidth', bandwidth: 128000 },
  audio_language: 'pt-BR',
  include_video: true,
  include_audio: false,
  download_concurrency: 2,
  manifest_timeout_ms: 3000,
  min_refresh_interval_ms: 500,
  max_refresh_retries: 0,
  segment_timeout_ms: 8000,
  max_segment_retries: 0,
  live_start: { type: 'delay', delay_ms: 6000 },
};

const schemas = [
  ['full config', MesioConfigSchema],
  ['override', MesioConfigOverrideSchema],
] as const;

describe('Mesio DASH configuration', () => {
  it('defaults DASH fixing on and leaves DASH settings to the engine', () => {
    const parsed = MesioConfigSchema.parse({});
    expect(parsed.fix_dash).toBe(true);
    expect(parsed).not.toHaveProperty('dash');
    expect(MesioConfigSchema.parse({ dash: null }).dash).toBeUndefined();
    expect(MesioConfigOverrideSchema.parse({})).toEqual({});
  });

  it.each(schemas)(
    'round-trips every setting in the %s schema',
    (_, schema) => {
      const config = { fix_dash: false, dash: full };
      const saved = JSON.parse(JSON.stringify(schema.parse(config)));
      expect(saved).toMatchObject(config);
      expect(schema.parse(saved)).toMatchObject(config);
    },
  );

  // A template override refuses an emptied field instead: there, an input only shows for a
  // custom value (see mesio-override.test.ts).
  it.each(schemas.slice(0, 1))(
    'drops empty form fields in the %s schema instead of sending them',
    (_, schema) => {
      const dash = {
        audio_language: '  ',
        download_concurrency: '',
        segment_timeout_ms: '',
        include_video: undefined,
        video_quality: undefined,
      };
      const saved = JSON.parse(JSON.stringify(schema.parse({ dash })));
      expect(saved).not.toHaveProperty('dash');
      expect(schema.parse({ dash: { download_concurrency: '' } })).toEqual(
        schema.parse({}),
      );
    },
  );

  it('coerces numbers typed into the form and trims the language', () => {
    const parsed = MesioConfigSchema.parse({
      dash: {
        audio_language: ' en ',
        download_concurrency: '3',
        video_quality: { type: 'closest_bandwidth', bandwidth: '2500000' },
        live_start: { type: 'delay', delay_ms: '0' },
      },
    });
    expect(parsed.dash).toMatchObject({
      audio_language: 'en',
      download_concurrency: 3,
      video_quality: { type: 'closest_bandwidth', bandwidth: 2500000 },
      live_start: { type: 'delay', delay_ms: 0 },
    });
  });

  it('keeps partial overrides partial and lets a template reset DASH settings', () => {
    const partial = { dash: { include_audio: false } };
    expect(MesioConfigOverrideSchema.parse(partial)).toEqual(partial);
    expect(EngineConfigOverrideSchema.parse(partial)).toEqual(partial);
    expect(EngineConfigOverrideSchema.parse({ fix_dash: false })).toEqual({
      fix_dash: false,
    });
    expect(MesioConfigOverrideSchema.parse({ dash: null })).toEqual({
      dash: null,
    });
  });

  it('rejects unknown keys in a DASH override', () => {
    expect(
      MesioConfigOverrideSchema.safeParse({ dash: { include_vidoe: false } })
        .success,
    ).toBe(false);
  });

  it.each(schemas)(
    'rejects settings the engine refuses in the %s schema',
    (_, schema) => {
      for (const dash of [
        { include_video: false, include_audio: false },
        { download_concurrency: 0 },
        { manifest_timeout_ms: 0 },
        { min_refresh_interval_ms: 0 },
        { segment_timeout_ms: 0 },
        { segment_timeout_ms: 1.5 },
        { max_segment_retries: -1 },
        { max_refresh_retries: 4294967296 },
        { video_quality: { type: 'max_height', height: 0 } },
        { video_quality: { type: 'max_height', height: 4294967296 } },
        { video_quality: { type: 'closest_bandwidth', bandwidth: 0 } },
        { video_quality: { type: 'closest_bandwidth', bandwidth: '' } },
        { video_quality: { type: 'best' } },
        { audio_quality: { type: 'closest_bandwidth', bandwidth: 0 } },
        { audio_quality: { type: 'max_height', height: 720 } },
        { live_start: { type: 'delay', delay_ms: -1 } },
        { audio_language: 'english please' },
        { audio_language: 'toolongsubtag' },
        { audio_language: 'en-' },
      ]) {
        const result = schema.safeParse({ dash });
        expect(result.success, JSON.stringify(dash)).toBe(false);
      }
    },
  );

  it('reports a missing track on the audio switch', () => {
    const result = MesioConfigSchema.safeParse({
      dash: { include_video: false, include_audio: false },
    });
    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.path).toEqual(['dash', 'include_audio']);
  });
});

describe('Mesio DASH override semantics', () => {
  const resets = {
    video_quality: null,
    audio_quality: null,
    audio_language: null,
    include_video: null,
    include_audio: null,
    download_concurrency: null,
    manifest_timeout_ms: null,
    min_refresh_interval_ms: null,
    max_refresh_retries: null,
    segment_timeout_ms: null,
    max_segment_retries: null,
    live_start: null,
  };

  it('keeps a null reset of every setting in an override', () => {
    const override = { fix_dash: null, dash: resets };
    const saved = JSON.parse(
      JSON.stringify(MesioConfigOverrideSchema.parse(override)),
    );
    expect(saved).toEqual(override);
    expect(EngineConfigOverrideSchema.parse(override)).toEqual(override);
  });

  it('treats null as unset in an engine configuration', () => {
    const saved = JSON.parse(
      JSON.stringify(MesioConfigSchema.parse({ dash: resets })),
    );
    expect(saved).not.toHaveProperty('dash');
  });

  it('merges tracks like the backend: absent inherits, null resets', () => {
    const engine = resolveDashSettings({ include_video: false });
    expect(engine).toMatchObject({ include_video: false, include_audio: true });
    expect(
      dashOverrideDisablesBothTracks(engine, { include_audio: false }),
    ).toBe(true);
    expect(
      dashOverrideDisablesBothTracks(engine, {
        include_audio: false,
        include_video: null,
      }),
    ).toBe(false);
    expect(dashOverrideDisablesBothTracks(engine, null)).toBe(false);
    expect(dashOverrideDisablesBothTracks(engine, undefined)).toBe(false);
  });

  it('checks only Mesio overrides against their own engine', () => {
    const engines = [
      {
        id: 'a',
        engine_type: 'MESIO',
        config: { dash: { include_video: false } },
      },
      { id: 'b', engine_type: 'MESIO', config: {} },
      { id: 'c', engine_type: 'FFMPEG', config: {} },
    ];
    const dash = { include_audio: false };
    expect(
      overridesWithoutDashTracks(
        { a: { dash }, b: { dash }, c: { dash }, missing: { dash } },
        engines,
      ),
    ).toEqual(['a']);
  });
});
