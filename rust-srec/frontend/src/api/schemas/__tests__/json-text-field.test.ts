import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';

import { jsonTextField } from '../common';
import { PlatformConfigSchema } from '../platform';
import { StreamerSchema } from '../streamer';
import { GlobalConfigSchema } from '../system';
import { TemplateSchema } from '../template';

const schema = z.object({
  value: jsonTextField('value', z.object({ enabled: z.boolean() })),
});

// Every degraded value is reported, so the suite would otherwise be noisy.
const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

beforeEach(() => warn.mockClear());
afterAll(() => warn.mockRestore());

describe('jsonTextField', () => {
  it('decodes a stored JSON string', () => {
    expect(schema.parse({ value: '{"enabled":true}' })).toEqual({
      value: { enabled: true },
    });
  });

  it('accepts a value that is already decoded', () => {
    expect(schema.parse({ value: { enabled: false } })).toEqual({
      value: { enabled: false },
    });
  });

  it.each([
    ['text that is not JSON', 'not json at all'],
    ['a truncated object', '{"enabled":'],
    ['an empty string', ''],
    ['whitespace', '   '],
    ['JSON of the wrong shape', '{"enabled":"yes"}'],
    ['an explicit null', null],
  ])('reports %s as null instead of throwing', (_label, value) => {
    expect(() => schema.parse({ value })).not.toThrow();
    expect(schema.parse({ value })).toEqual({ value: null });
  });

  it('leaves an absent field absent', () => {
    expect(schema.parse({})).toEqual({});
  });

  // Saving the row afterwards overwrites what was there, so the discarded
  // value has to leave a trace.
  it('names the field it discarded', () => {
    schema.parse({ value: '{"enabled":' });
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining('"value"'),
      expect.anything(),
    );
  });

  it('reports the schema issues when the JSON is the wrong shape', () => {
    schema.parse({ value: '{"enabled":"yes"}' });
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining('"value"'),
      expect.arrayContaining([expect.objectContaining({ path: ['enabled'] })]),
    );
  });

  it('says nothing about a value it could read', () => {
    schema.parse({ value: '{"enabled":true}' });
    expect(warn).not.toHaveBeenCalled();
  });

  // The backend stores the JSON text `null` for an unconfigured column, so
  // that is the expected value and not something to report.
  it('treats stored JSON null as unconfigured without warning', () => {
    expect(schema.parse({ value: 'null' })).toEqual({ value: null });
    expect(warn).not.toHaveBeenCalled();
  });
});

// One unreadable row used to reject the entire list response, leaving the
// platforms and templates pages blank.
describe('config rows with unreadable JSON columns', () => {
  it('still parses a platform whose stored JSON is malformed', () => {
    const parsed = PlatformConfigSchema.parse({
      id: 'platform-1',
      name: 'Douyin',
      stream_selection_config: '{"preferred_formats":',
      danmu_statistics: 'null-ish garbage',
      download_retry_policy: '{"max_retries":"many"}',
      proxy_route: { kind: 'direct' },
      pipeline: '[not json]',
      session_complete_pipeline: '{',
      paired_segment_pipeline: 'undefined',
      platform_specific_config: '{"douyin":',
    });

    expect(parsed.name).toBe('Douyin');
    expect(parsed.stream_selection_config).toBeNull();
    expect(parsed.danmu_statistics).toBeNull();
    expect(parsed.download_retry_policy).toBeNull();
    expect(parsed.proxy_route).toEqual({ kind: 'direct' });
    expect(parsed.pipeline).toBeNull();
    expect(parsed.session_complete_pipeline).toBeNull();
    expect(parsed.paired_segment_pipeline).toBeNull();
    expect(parsed.platform_specific_config).toBeNull();
  });

  it('still parses a template whose stored JSON is malformed', () => {
    const parsed = TemplateSchema.parse({
      id: 'template-1',
      name: 'Archive',
      stream_selection_config: '{"preferred_qualities":',
      danmu_statistics: 'nope',
      download_retry_policy: '{',
      proxy_route: { kind: 'proxy', id: 'proxy-1' },
      pipeline: 'not json',
      session_complete_pipeline: '{',
      paired_segment_pipeline: '[',
      platform_overrides: '{"douyin":',
      engines_override: '{"hls":',
    });

    expect(parsed.name).toBe('Archive');
    expect(parsed.stream_selection_config).toBeNull();
    expect(parsed.danmu_statistics).toBeNull();
    expect(parsed.download_retry_policy).toBeNull();
    expect(parsed.proxy_route).toEqual({ kind: 'proxy', id: 'proxy-1' });
    expect(parsed.pipeline).toBeNull();
    expect(parsed.session_complete_pipeline).toBeNull();
    expect(parsed.paired_segment_pipeline).toBeNull();
    expect(parsed.platform_overrides).toBeNull();
    expect(parsed.engines_override).toBeNull();
  });

  it('keeps the readable columns of a row that has one bad one', () => {
    const parsed = TemplateSchema.parse({
      id: 'template-2',
      name: 'Mixed',
      stream_selection_config: '{"preferred_qualities":["source"]}',
      danmu_statistics: '{"enabled":',
    });

    expect(parsed.stream_selection_config).toEqual({
      preferred_qualities: ['source'],
    });
    expect(parsed.danmu_statistics).toBeNull();
  });
});

// Stored configs hold values the backend reads leniently. One of them used to
// reject the whole streamers or platforms list, or blank the global config page.
describe('stored configs the backend tolerates', () => {
  it('reads them as the backend resolves them', () => {
    const parsed = StreamerSchema.parse({
      id: 'streamer-1',
      name: 'Streamer',
      url: 'https://example.com/live',
      platform_config_id: 'platform-1',
      state: 'NOT_LIVE',
      priority: 'NORMAL',
      enabled: true,
      consecutive_error_count: 0,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      streamer_specific_config: {
        extractor: 'Streamlink',
        offline_check_count: 0,
        offline_check_delay_ms: 10,
        download_retry_policy: { max_retries: 5 },
        stream_selection_config: { preferred_formats: null },
        pipeline: {
          name: 'p',
          steps: [
            {
              id: 'remux',
              step: { type: 'preset', name: 'remux' },
              retry: null,
              timeout_secs: null,
            },
          ],
        },
      },
    });

    expect(parsed.streamer_specific_config).toMatchObject({
      extractor: 'streamlink',
      offline_check_count: 1,
      offline_check_delay_ms: 1000,
      download_retry_policy: { max_retries: 5, initial_delay_ms: 1000 },
      stream_selection_config: { preferred_formats: null },
      pipeline: { steps: [{ id: 'remux', retry: null }] },
    });

    const platform = PlatformConfigSchema.parse({
      id: 'platform-1',
      name: 'Douyin',
      extractor: ' Streamlink ',
      download_retry_policy: '{"max_retries":5}',
    });
    expect(platform.extractor).toBe('streamlink');
    expect(platform.download_retry_policy).toMatchObject({ max_retries: 5 });
    expect(
      TemplateSchema.parse({ id: 'template-1', name: 'T', extractor: 'ytdlp' })
        .extractor,
    ).toBeNull();
    expect(GlobalConfigSchema.shape.default_extractor.parse('AUTO')).toBe(
      'auto',
    );
  });
});
