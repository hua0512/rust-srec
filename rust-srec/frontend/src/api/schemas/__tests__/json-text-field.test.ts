import { describe, expect, it } from 'vitest';
import { z } from 'zod';

import { jsonTextField } from '../common';
import { PlatformConfigSchema } from '../platform';
import { TemplateSchema } from '../template';

const schema = z.object({
  value: jsonTextField(z.object({ enabled: z.boolean() })),
});

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
      proxy_config: '',
      pipeline: '[not json]',
      session_complete_pipeline: '{',
      paired_segment_pipeline: 'undefined',
      platform_specific_config: '{"douyin":',
    });

    expect(parsed.name).toBe('Douyin');
    expect(parsed.stream_selection_config).toBeNull();
    expect(parsed.danmu_statistics).toBeNull();
    expect(parsed.download_retry_policy).toBeNull();
    expect(parsed.proxy_config).toBeNull();
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
      proxy_config: '{"enabled":"maybe"}',
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
    expect(parsed.proxy_config).toBeNull();
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
      proxy_config: '{"enabled":true,"url":"http://proxy.example"}',
      danmu_statistics: '{"enabled":',
    });

    expect(parsed.proxy_config).toMatchObject({
      enabled: true,
      url: 'http://proxy.example',
    });
    expect(parsed.danmu_statistics).toBeNull();
  });
});
